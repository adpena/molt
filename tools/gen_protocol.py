#!/usr/bin/env python3
"""Generate the SimpleTIRGenerator static-typing Protocol shim (F1 decomposition).

Single source of truth: the *assembled* ``SimpleTIRGenerator`` class and its
visitor/lowering mixins (``src/molt/frontend/__init__.py`` +
``src/molt/frontend/visitors/*.py`` + ``src/molt/frontend/lowering/*.py``), plus
the curated attribute-type table that already lives in the two generated files.

Why this generator exists
=========================
The god-class ``SimpleTIRGenerator`` was decomposed (move-only) into a package of
mixins composed via MRO. Each mixin annotates ``self`` as ``_GeneratorProtocol``
under ``TYPE_CHECKING`` (``if TYPE_CHECKING: _MixinBase = _GeneratorProtocol`` /
``else: _MixinBase = object``) so that cross-mixin ``self.<method>`` /
``self.<attr>`` references type-check across files - the guarantee the single
class form had implicitly. That guarantee only holds while the Protocol is a
SUPERSET of the assembled class's real method+attribute surface; if a method
moves into a mixin and the Protocol is not regenerated, the moved method - and
every sibling-mixin call to it - silently loses static checking.

``tests/test_frontend_package_composition.py`` pins that superset invariant
(``test_protocol_covers_full_class_method_surface`` /
``test_protocol_covers_full_class_attr_surface`` /
``test_every_mixin_method_is_on_protocol``). This generator is what keeps it
green: a generated file with no committed generator is structural debt.

The shim is import-only under ``TYPE_CHECKING`` - it has NO runtime effect.
Regenerating it cannot change behavior or TIR output; the gate is the test
suite, not byte-identical TIR.

What it emits
=============
  - ``src/molt/frontend/_protocol_attrs.py`` - ``_GeneratorProtocolAttrs``
    (Protocol): the first half (alphabetical) of the attribute surface.
  - ``src/molt/frontend/_protocol.py`` - ``_GeneratorProtocol``
    (``_GeneratorProtocolAttrs``, Protocol): the second half of the attribute
    surface, followed by every method signature.

Determinism / clean diffs
==========================
  * Methods are emitted SORTED by name (dedup: most-derived MRO definition wins),
    with their REAL signature extracted from source AST (decorators + ``def``
    header preserved verbatim, body replaced with ``...``). Method signatures
    therefore need no curated input - they are pure introspection.
  * Attributes are emitted SORTED by name and split at the midpoint across the
    two files. Their *types* come from the curated table harvested from the two
    files (the only place 191 of the 195 attribute types are recorded - they are
    set via ``self.x = ...`` in ``__init__`` with no source annotation), merged
    with the 4 class-level ``__annotations__``. A brand-new attribute introduced
    by a future move that has no curated type defaults to ``Any`` (its NAME is
    still on the Protocol, so the superset test passes; ``--check`` then shows a
    diff so a human can refine the ``Any`` to a precise type).
  * Imports are computed from the identifiers actually referenced in the emitted
    signatures/annotations, so a new ``_types`` type used in a moved signature is
    auto-imported (no fragile hand-maintained import list).

Usage::

    python3 tools/gen_protocol.py            # (re)write the generated files
    python3 tools/gen_protocol.py --check    # exit 1 if a generated file is stale
"""

from __future__ import annotations

import argparse
import ast
import inspect
import sys
import textwrap
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

OUT_PROTOCOL = ROOT / "src/molt/frontend/_protocol.py"
OUT_ATTRS = ROOT / "src/molt/frontend/_protocol_attrs.py"

# Names that ``typing`` (re-)exports and that may appear in extracted signatures
# or the generated scaffold. ``Protocol`` / ``TYPE_CHECKING`` are always required
# by the scaffold; the rest are pulled in on demand from observed usage.
_TYPING_NAMES = {
    "Any",
    "Callable",
    "ClassVar",
    "Final",
    "Iterable",
    "Iterator",
    "Literal",
    "Mapping",
    "Optional",
    "Protocol",
    "Sequence",
    "TYPE_CHECKING",
    "Tuple",
    "Union",
}

# Imports that are only valid under ``if TYPE_CHECKING`` (avoid an import cycle /
# runtime cost): identifier -> ``from`` module.
_TYPE_CHECKING_IMPORTS = {
    "SemaResult": "molt.frontend.sema",
    "TypeFacts": "molt.type_facts",
}

# ast.NodeVisitor's traversal dispatch is part of the self surface used by the
# mixins; the rest of NodeVisitor remains builtin/base-class noise.
_NODE_VISITOR_DISPATCH_METHODS = {"generic_visit", "visit"}

# Stdlib modules that ``_types`` happens to re-export (it imports them at module
# scope) but which the generated files import directly by their own line. They
# must never be pulled from the ``from molt.frontend._types import (...)`` block.
_STDLIB_MODULE_NAMES = {"ast"}


class ProtocolGenError(RuntimeError):
    pass


# ---------------------------------------------------------------------------
# Surface introspection (mirrors tests/test_frontend_package_composition.py)
# ---------------------------------------------------------------------------


def _load_generator() -> type:
    """Import and return the assembled ``SimpleTIRGenerator`` class.

    Importing it pulls every visitor/lowering mixin into its MRO, which is the
    exact surface the Protocol must cover.
    """
    from molt.frontend import SimpleTIRGenerator

    return SimpleTIRGenerator


def _surface_classes(generator: type) -> list[type]:
    """The MRO classes that contribute to the generator's own surface.

    Excludes ``object``. Keeps ``ast.NodeVisitor`` so `visit` and
    `generic_visit` remain in the Protocol; visitor mixins call those dispatch
    methods through `self`.
    """
    return [k for k in generator.__mro__ if k is not object]


def _builtin_names() -> set[str]:
    """Base names not part of the generator's own protocol surface."""
    return set(dir(object))


def _unwrap(value: object) -> object:
    """Return the underlying function for a ``staticmethod`` / ``classmethod``."""
    if isinstance(value, (staticmethod, classmethod)):
        return value.__func__
    return value


# ---------------------------------------------------------------------------
# Method signature extraction
# ---------------------------------------------------------------------------


def _function_def_source(func: object) -> str | None:
    """Return the dedented source of *func* (or ``None`` if unavailable)."""
    try:
        raw = inspect.getsource(func)  # type: ignore[arg-type]
    except (OSError, TypeError):
        return None
    return textwrap.dedent(raw)


def _render_method_stub(name: str, value: object) -> str | None:
    """Render a single Protocol method stub for *name* from its real source.

    Preserves the decorator list (``@staticmethod`` / ``@classmethod`` /
    ``@property`` etc.) and the full ``def`` header (all parameter and return
    annotations verbatim), replacing the body with ``...``. Returns ``None`` if
    the source cannot be parsed (the caller then skips it - never silently emits
    a wrong signature).
    """
    func = _unwrap(value)
    src = _function_def_source(func)
    if src is None:
        return None
    try:
        module = ast.parse(src)
    except SyntaxError:
        return None
    func_node: ast.FunctionDef | ast.AsyncFunctionDef | None = None
    for node in module.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            func_node = node
            break
    if func_node is None or func_node.name != name:
        return None

    # Rebuild decorators from the *binding* (vars() value), not the AST: the AST
    # decorator list can include project-specific decorators that are not part of
    # the typing surface, while staticmethod/classmethod are reliably visible on
    # the binding. We emit exactly the two binding decorators the Protocol needs.
    decorators: list[str] = []
    if isinstance(value, staticmethod):
        decorators.append("    @staticmethod")
    elif isinstance(value, classmethod):
        decorators.append("    @classmethod")
    elif isinstance(value, property):
        decorators.append("    @property")

    # Re-render the signature deterministically with ast.unparse, then strip the
    # body to ``...``. ast.unparse normalizes whitespace, giving stable diffs
    # regardless of how the source wrapped its parameters.
    stripped = (
        ast.AsyncFunctionDef
        if isinstance(func_node, ast.AsyncFunctionDef)
        else ast.FunctionDef
    )
    rebuilt = stripped(
        name=func_node.name,
        args=func_node.args,
        body=[ast.Expr(value=ast.Constant(value=Ellipsis))],
        decorator_list=[],
        returns=func_node.returns,
        type_comment=None,
        type_params=getattr(func_node, "type_params", []),
    )
    ast.fix_missing_locations(rebuilt)
    header = ast.unparse(rebuilt)
    # ``ast.unparse`` emits "def f(...):\n    ...". Collapse the trailing body
    # onto the signature so generated diffs stay dense and reviewable.
    if header.endswith("\n    ..."):
        header = header[: -len("\n    ...")] + " ..."
    indented = textwrap.indent(header, "    ")
    out = "\n".join(decorators + [indented]) if decorators else indented
    return out


def _collect_methods(
    surface_classes: list[type], builtins: set[str]
) -> list[tuple[str, str]]:
    """Collect ``(name, rendered_stub)`` for every method on the generator's
    surface, deduplicated most-derived-wins, sorted by name.

    A method defined in several MRO classes (genuine override) is rendered once,
    from the most-derived class (first in MRO order) - the binding the runtime
    actually resolves.
    """
    chosen: dict[str, object] = {}
    for klass in surface_classes:  # MRO order == most-derived first
        for attr_name, value in vars(klass).items():
            if attr_name.startswith("__") and attr_name != "__init__":
                continue
            if (
                klass is ast.NodeVisitor
                and attr_name not in _NODE_VISITOR_DISPATCH_METHODS
            ):
                continue
            if attr_name in builtins and attr_name != "__init__":
                continue
            if not callable(_unwrap(value)):
                continue
            chosen.setdefault(attr_name, value)

    out: list[tuple[str, str]] = []
    unresolved: list[str] = []
    for name in sorted(chosen):
        stub = _render_method_stub(name, chosen[name])
        if stub is None:
            unresolved.append(name)
            continue
        out.append((name, stub))
    if unresolved:
        raise ProtocolGenError(
            "could not extract a real signature for these methods (source "
            f"unavailable / unparsable): {unresolved}. A generated Protocol must "
            "carry real signatures - fix the source or this generator before "
            "emitting a degraded stub."
        )
    return out


# ---------------------------------------------------------------------------
# Attribute surface + curated type table
# ---------------------------------------------------------------------------


def _init_store_attrs(generator: type) -> set[str]:
    """Instance attributes assigned via ``self.x = ...`` in ``__init__``.

    Mirrors the AST walk in the coverage test exactly so the generated name set
    is the same one the test computes.
    """
    attrs: set[str] = set()
    init_src = textwrap.dedent(inspect.getsource(generator.__init__))
    for node in ast.walk(ast.parse(init_src)):
        if (
            isinstance(node, ast.Attribute)
            and isinstance(node.value, ast.Name)
            and node.value.id == "self"
            and isinstance(node.ctx, ast.Store)
        ):
            attrs.add(node.attr)
    return attrs


def _class_annotation_table(surface_classes: list[type]) -> dict[str, str]:
    """The class-level ``__annotations__`` across the surface (name -> type str).

    ``from __future__ import annotations`` makes these values strings already.
    Most-derived wins on conflict (MRO order).
    """
    table: dict[str, str] = {}
    for klass in surface_classes:
        for name, annotation in getattr(klass, "__annotations__", {}).items():
            text = (
                annotation
                if isinstance(annotation, str)
                else _annotation_to_text(annotation)
            )
            table.setdefault(name, text)
    return table


def _annotation_to_text(annotation: object) -> str:
    """Best-effort stable string for a non-string annotation object."""
    if isinstance(annotation, type):
        return annotation.__name__
    return str(annotation)


def _harvest_curated_attr_types(paths: list[Path]) -> dict[str, str]:
    """Parse the existing generated files and return ``name -> annotation text``
    for every class-body attribute annotation found.

    This is the authoritative source for the ~191 attributes that are only ever
    set via ``self.x = ...`` in ``__init__`` (no source annotation) and whose
    precise types are recorded ONLY here. Re-parsing before overwriting keeps
    those curated types stable across regenerations (idempotent).
    """
    curated: dict[str, str] = {}
    for path in paths:
        if not path.exists():
            continue
        tree = ast.parse(path.read_text(encoding="utf-8"))
        for node in tree.body:
            if not isinstance(node, ast.ClassDef):
                continue
            for stmt in node.body:
                if (
                    isinstance(stmt, ast.AnnAssign)
                    and isinstance(stmt.target, ast.Name)
                    and stmt.annotation is not None
                ):
                    curated[stmt.target.id] = ast.unparse(stmt.annotation)
    return curated


def _collect_attrs(
    generator: type, surface_classes: list[type], builtins: set[str]
) -> list[tuple[str, str]]:
    """Collect ``(name, annotation_text)`` for the full attribute surface,
    sorted by name. Types come from (class ``__annotations__`` union curated table),
    with ``Any`` as the explicit fallback for a name that has no recorded type.
    """
    names = _init_store_attrs(generator)
    for klass in surface_classes:
        names.update(getattr(klass, "__annotations__", {}))
    names -= builtins

    class_table = _class_annotation_table(surface_classes)
    curated = _harvest_curated_attr_types([OUT_ATTRS, OUT_PROTOCOL])

    out: list[tuple[str, str]] = []
    for name in sorted(names):
        # Class-level source annotation is the strongest signal (it is real,
        # current source); fall back to the curated table, then to ``Any``.
        annotation = class_table.get(name) or curated.get(name) or "Any"
        out.append((name, annotation))
    return out


# ---------------------------------------------------------------------------
# Import computation
# ---------------------------------------------------------------------------


def _referenced_identifiers(annotation_texts: list[str]) -> set[str]:
    """Root identifiers referenced by a list of annotation/signature snippets.

    Each snippet is parsed as Python; we collect the *root* ``Name`` of every
    expression (the head of an attribute chain, e.g. ``ast`` in ``ast.Module``)
    plus bare names. String-literal forward refs (``"MoltValue | None"``) are
    recursively parsed so their identifiers are captured too.
    """
    found: set[str] = set()

    def visit_expr(text: str) -> None:
        try:
            tree = ast.parse(text, mode="eval")
        except SyntaxError:
            return
        for node in ast.walk(tree):
            if isinstance(node, ast.Name):
                found.add(node.id)
            elif isinstance(node, ast.Attribute):
                root = node
                while isinstance(root, ast.Attribute):
                    root = root.value  # type: ignore[assignment]
                if isinstance(root, ast.Name):
                    found.add(root.id)
            elif isinstance(node, ast.Constant) and isinstance(node.value, str):
                # Forward-ref string inside the annotation.
                visit_expr(node.value)

    for text in annotation_texts:
        visit_expr(text)
    return found


def _annotation_texts_from_methods(methods: list[tuple[str, str]]) -> list[str]:
    """Extract every annotation + default expression text from rendered method
    stubs so import computation sees the full type vocabulary they use."""
    texts: list[str] = []
    for _name, stub in methods:
        # Re-parse the rendered stub; collect arg/return annotations + defaults.
        try:
            tree = ast.parse(textwrap.dedent(stub))
        except SyntaxError:
            continue
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                args = node.args
                for arg in (
                    *args.posonlyargs,
                    *args.args,
                    *args.kwonlyargs,
                    args.vararg,
                    args.kwarg,
                ):
                    if arg is not None and arg.annotation is not None:
                        texts.append(ast.unparse(arg.annotation))
                if node.returns is not None:
                    texts.append(ast.unparse(node.returns))
                for default in (*args.defaults, *args.kw_defaults):
                    if default is not None:
                        texts.append(ast.unparse(default))
    return texts


def _compute_imports(
    annotation_texts: list[str], *, types_module_exports: set[str]
) -> tuple[list[str], list[str], list[str], bool]:
    """Return (typing_names, types_names, type_checking_lines, needs_ast).

    * typing_names: sorted ``typing`` symbols to import (always includes the
      scaffold-required ``Protocol`` / ``TYPE_CHECKING`` / ``Any``).
    * types_names: sorted ``molt.frontend._types`` symbols referenced.
    * type_checking_lines: ``from <mod> import <name>`` lines for the
      TYPE_CHECKING-only symbols referenced.
    * needs_ast: whether ``ast`` is referenced (``import ast``).
    """
    referenced = _referenced_identifiers(annotation_texts)

    typing_names = {"Protocol", "TYPE_CHECKING", "Any"}
    typing_names |= referenced & _TYPING_NAMES

    # ``_types`` re-exports several ``typing`` symbols (``Any``/``Literal``/...)
    # and the stdlib ``ast`` module (it does ``import ast`` at module scope), all
    # of which are imported by their own dedicated lines above. Subtract them (and
    # the TYPE_CHECKING-only names, emitted in their own block) so ``_types`` never
    # emits a duplicate / shadowing import.
    types_names = (
        (referenced & types_module_exports)
        - _TYPING_NAMES
        - set(_TYPE_CHECKING_IMPORTS)
        - _STDLIB_MODULE_NAMES
    )

    tc_lines: list[str] = []
    for name in sorted(referenced & set(_TYPE_CHECKING_IMPORTS)):
        tc_lines.append(f"    from {_TYPE_CHECKING_IMPORTS[name]} import {name}")

    needs_ast = "ast" in referenced
    return sorted(typing_names), sorted(types_names), tc_lines, needs_ast


# ---------------------------------------------------------------------------
# File rendering
# ---------------------------------------------------------------------------

_DO_NOT_EDIT = (
    "# @generated by tools/gen_protocol.py - DO NOT EDIT.\n"
    "# Run `python3 tools/gen_protocol.py` to regenerate from the assembled\n"
    "# SimpleTIRGenerator class + its visitor/lowering mixins. `--check` (CI)\n"
    "# exits 1 if this file is stale. This module is import-only under\n"
    "# TYPE_CHECKING; it has no runtime effect.\n"
)


def _render_import_block(
    typing_names: list[str],
    types_names: list[str],
    tc_lines: list[str],
    needs_ast: bool,
    *,
    extra_imports: list[str] = (),
) -> str:
    lines: list[str] = ["from __future__ import annotations", ""]
    if needs_ast:
        lines.append("import ast")
    lines.append("from typing import (")
    for name in typing_names:
        lines.append(f"    {name},")
    lines.append(")")
    lines.append("")
    for extra in extra_imports:
        lines.append(extra)
    if extra_imports:
        lines.append("")
    if types_names:
        lines.append("from molt.frontend._types import (")
        for name in types_names:
            lines.append(f"    {name},")
        lines.append(")")
        lines.append("")
    if tc_lines:
        lines.append("if TYPE_CHECKING:")
        lines.extend(tc_lines)
        lines.append("")
    return "\n".join(lines)


def _render_attrs_block(attrs: list[tuple[str, str]]) -> str:
    if not attrs:
        return "    pass\n"
    return "".join(f"    {name}: {annotation}\n" for name, annotation in attrs)


def render_attrs_file(
    attrs_first_half: list[tuple[str, str]],
    *,
    types_module_exports: set[str],
) -> str:
    annotation_texts = [a for _n, a in attrs_first_half]
    typing_names, types_names, tc_lines, needs_ast = _compute_imports(
        annotation_texts, types_module_exports=types_module_exports
    )
    header = (
        '"""Static-typing Protocol attribute base for the SimpleTIRGenerator surface.\n\n'
        "GENERATED - see tools/gen_protocol.py. This holds the first (alphabetical)\n"
        "half of the assembled generator's attribute surface as a Protocol base; the\n"
        "second half and every method signature live in ``_protocol.py``. Splitting\n"
        "the surface across two files keeps each file reviewable.\n\n"
        "Import-only under TYPE_CHECKING; no runtime effect.\n"
        '"""\n\n'
    )
    imports = _render_import_block(typing_names, types_names, tc_lines, needs_ast)
    body = "\nclass _GeneratorProtocolAttrs(Protocol):\n" + _render_attrs_block(
        attrs_first_half
    )
    return _DO_NOT_EDIT + "\n" + header + imports + body


def render_protocol_file(
    attrs_second_half: list[tuple[str, str]],
    methods: list[tuple[str, str]],
    *,
    types_module_exports: set[str],
) -> str:
    annotation_texts = [a for _n, a in attrs_second_half]
    annotation_texts.extend(_annotation_texts_from_methods(methods))
    typing_names, types_names, tc_lines, needs_ast = _compute_imports(
        annotation_texts, types_module_exports=types_module_exports
    )
    header = (
        '"""Static-typing Protocol for SimpleTIRGenerator (F1 decomposition).\n\n'
        "GENERATED - see tools/gen_protocol.py. Enumerates the full method +\n"
        "attribute surface of the assembled generator so that visitor/lowering\n"
        "mixins can annotate ``self`` as ``_GeneratorProtocol`` and have cross-mixin\n"
        "``self.<method>`` / ``self.<attr>`` references type-check (the single-class\n"
        "form had this implicitly; the Protocol restores it across files).\n\n"
        "This module is import-only under TYPE_CHECKING; it has no runtime effect.\n"
        '"""\n\n'
    )
    extra = ["from molt.frontend._protocol_attrs import _GeneratorProtocolAttrs"]
    imports = _render_import_block(
        typing_names, types_names, tc_lines, needs_ast, extra_imports=extra
    )
    body_parts: list[str] = [
        "\nclass _GeneratorProtocol(_GeneratorProtocolAttrs, Protocol):\n"
    ]
    if attrs_second_half:
        body_parts.append(_render_attrs_block(attrs_second_half))
        body_parts.append("\n")
    for _name, stub in methods:
        body_parts.append(stub)
        body_parts.append("\n\n")
    body = "".join(body_parts).rstrip("\n") + "\n"
    return _DO_NOT_EDIT + "\n" + header + imports + body


def _split_attrs(
    attrs: list[tuple[str, str]],
) -> tuple[list[tuple[str, str]], list[tuple[str, str]]]:
    """Split the sorted attribute list at the midpoint (first half -> attrs base,
    second half -> main protocol). Deterministic ceil split."""
    midpoint = (len(attrs) + 1) // 2
    return attrs[:midpoint], attrs[midpoint:]


def generate() -> dict[Path, str]:
    """Render both generated files. Returns ``{path: rendered_text}``."""
    import molt.frontend._types as types_module

    types_module_exports = {n for n in dir(types_module) if not n.startswith("__")}

    generator = _load_generator()
    surface_classes = _surface_classes(generator)
    builtins = _builtin_names()

    methods = _collect_methods(surface_classes, builtins)
    attrs = _collect_attrs(generator, surface_classes, builtins)
    attrs_first, attrs_second = _split_attrs(attrs)

    attrs_text = render_attrs_file(
        attrs_first, types_module_exports=types_module_exports
    )
    protocol_text = render_protocol_file(
        attrs_second, methods, types_module_exports=types_module_exports
    )
    return {OUT_ATTRS: attrs_text, OUT_PROTOCOL: protocol_text}


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def _check(path: Path, rendered: str) -> bool:
    """Return True if *path* is in sync with *rendered* (prints a hint if not)."""
    if not path.exists():
        print(f"MISSING generated file: {path}", file=sys.stderr)
        return False
    current = path.read_text(encoding="utf-8")
    if current != rendered:
        print(
            f"STALE generated file: {path.relative_to(ROOT)}\n"
            "  run `python3 tools/gen_protocol.py` to regenerate from the "
            "assembled SimpleTIRGenerator surface",
            file=sys.stderr,
        )
        return False
    return True


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if a generated file is stale (CI mode); do not write",
    )
    args = ap.parse_args(argv)

    rendered = generate()

    if args.check:
        ok = True
        for path, text in rendered.items():
            ok = _check(path, text) and ok
        if ok:
            print("protocol generated files: in sync")
        return 0 if ok else 1

    for path, text in rendered.items():
        path.write_text(text, encoding="utf-8", newline="\n")
        print(f"wrote {path.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
