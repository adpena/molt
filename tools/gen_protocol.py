#!/usr/bin/env python3
"""Generate the SimpleTIRGenerator static-typing Protocol shim (F1 decomposition).

Single source of truth: the *assembled* ``SimpleTIRGenerator`` class and its
visitor/lowering mixins (``src/molt/frontend/__init__.py`` +
``src/molt/frontend/visitors/*.py`` + ``src/molt/frontend/lowering/*.py``), plus
the curated attribute-type table that already lives in the two generated files.

One invocation-scoped source index resolves the actual unwrapped runtime
functions to immutable source-file AST snapshots. Each source file is read and
parsed once; one method-body pass collects stores and annotations. Signatures,
decorators and imports use those same source facts without reparsing rendered
stubs. Composition coverage shares source identity/parsing only, keeping its
independent attribute visitor as a separate coverage oracle.

Why this generator exists
=========================
The god-class ``SimpleTIRGenerator`` was decomposed (move-only) into a package of
mixins composed via MRO. Each mixin derives from
``molt.frontend._mixin_base.GeneratorMixinBase`` - ``object`` at runtime, and
under ``TYPE_CHECKING`` ``ast.NodeVisitor`` followed by ``_GeneratorProtocol`` -
so that cross-mixin ``self.<method>`` / ``self.<attr>`` references type-check
across files - the guarantee the single class form had implicitly. That guarantee only holds while the Protocol is a
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
    with their call shape and annotations extracted from source AST. Defaults
    and bodies become ``...``: protocols describe optional arguments, not the
    implementation expressions that supply their values. Method signatures
    therefore need no curated input - they are pure introspection.
  * Attributes are emitted SORTED by name and split at the midpoint across the
    two files. Their *types* come from the curated table harvested from the two
    files (the only place most attribute types are recorded - they are set via
    direct ``self.x = ...`` assignments in the assembled generator/mixin method
    surface with no source annotation). Class-level ``__annotations__`` take
    precedence over explicit ``self.x: T`` annotations, then curated types.
    A brand-new attribute introduced by a future move that
    has no curated type defaults to ``Any`` (its NAME is still on the Protocol,
    so the superset test passes; ``--check`` then shows a diff so a human can
    refine the ``Any`` to a precise type).
  * Imports are computed from the identifiers actually referenced in the emitted
    signatures/annotations, so a new ``_types`` type used in a moved signature is
    auto-imported (no fragile hand-maintained import list).
  * Rendered output is passed through Ruff format before write/check so generated
    sync and repository formatting have one authority.

Usage::

    python3 tools/gen_protocol.py            # (re)write the generated files
    python3 tools/gen_protocol.py --check    # exit 1 if a generated file is stale
"""

from __future__ import annotations

import argparse
import ast
import contextlib
import copy
import subprocess
import sys
import textwrap
from dataclasses import dataclass
from pathlib import Path

from generator_io import generated_file_matches, write_generated_text

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.python_source_index import (  # noqa: E402 - direct-script path bootstrap above
    PythonFunctionSource,
    PythonSourceError,
    PythonSourceIndex,
)

OUT_PROTOCOL = ROOT / "src/molt/frontend/_protocol.py"
OUT_ATTRS = ROOT / "src/molt/frontend/_protocol_attrs.py"

# Names that ``typing`` (re-)exports and that may appear in extracted signatures
# or the generated scaffold. ``Protocol`` / ``TYPE_CHECKING`` are always required
# by the scaffold; the rest are pulled in on demand from observed usage.
_TYPING_NAMES = {
    "Any",
    "AsyncIterator",
    "Callable",
    "ClassVar",
    "Collection",
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
    "FunctionKind": "molt.frontend.sema",
    "StatefulFunctionFramePlan": "molt.frontend.sema.funcmeta",
    "ModuleExecutionKind": "molt.compiler_analysis.python_imports",
    "ModuleImportContext": "molt.compiler_analysis.python_imports",
    "ModuleImportFlow": "molt.compiler_analysis.python_imports",
    "PythonBindingIndex": "molt.compiler_analysis.python_binding_facts",
    "PythonDependencyAuthority": "molt.compiler_analysis.python_lexical_scope",
    "PythonFrameContextScope": "molt.frontend.lowering.function_lifecycle",
    "SemaResult": "molt.frontend.sema",
    "SerializationContext": "molt.frontend.lowering.serialization_context",
    "StaticTruthKwargs": "molt.compiler_analysis.static_truth",
    "_ConditionMode": "molt.frontend.lowering.condition_flow",
    "_ConditionMerge": "molt.frontend.lowering.condition_flow",
    "_ConditionBindingState": "molt.frontend.lowering.condition_flow",
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


# ---------------------------------------------------------------------------
# Method signature extraction
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class MethodBody:
    attrs: frozenset[str]
    annotations: tuple[tuple[str, str], ...]


@dataclass(frozen=True)
class MethodSurface:
    name: str
    node: ast.FunctionDef | ast.AsyncFunctionDef
    decorators: tuple[str, ...]
    annotation_texts: tuple[str, ...]
    body: MethodBody


@dataclass(frozen=True)
class ProtocolSurface:
    methods: tuple[MethodSurface, ...]
    attrs: tuple[tuple[str, str], ...]


def _method_surface(
    name: str, value: object, source: PythonFunctionSource, body: MethodBody
) -> MethodSurface:
    """Project one selected runtime binding without changing its source AST."""
    func_node = source.node
    if func_node.name != name:
        raise ProtocolGenError(
            f"method binding {name!r} does not match source definition "
            f"{func_node.name!r} at {source.path}:{func_node.lineno}"
        )

    # Binding decorators and contextlib wrappers change the callable contract.
    # Resolve contextlib decorators by identity, including import aliases, so a
    # generator method is not incorrectly projected as a bare Iterator.
    decorators: list[str] = []
    if isinstance(value, staticmethod):
        decorators.append("staticmethod")
    elif isinstance(value, classmethod):
        decorators.append("classmethod")
    namespace = source.function.__globals__
    for decorator in func_node.decorator_list:
        binding = None
        if isinstance(decorator, ast.Name):
            binding = namespace.get(decorator.id)
        elif (
            isinstance(decorator, ast.Attribute)
            and isinstance(decorator.value, ast.Name)
            and namespace.get(decorator.value.id) is contextlib
        ):
            binding = getattr(contextlib, decorator.attr, None)
        if binding is contextlib.contextmanager:
            decorators.append("contextmanager")
        elif binding is contextlib.asynccontextmanager:
            decorators.append("asynccontextmanager")

    args = func_node.args
    annotations = tuple(
        ast.unparse(arg.annotation)
        for arg in (
            *args.posonlyargs,
            *args.args,
            *args.kwonlyargs,
            args.vararg,
            args.kwarg,
        )
        if arg is not None and arg.annotation is not None
    )
    if func_node.returns is not None:
        annotations += (ast.unparse(func_node.returns),)
    return MethodSurface(name, func_node, tuple(decorators), annotations, body)


def _render_method_stub(method: MethodSurface) -> str:
    """Render the already-resolved signature; source nodes remain immutable."""
    func_node = method.node

    # Re-render the signature deterministically with ast.unparse, then strip the
    # body to ``...``. ast.unparse normalizes whitespace, giving stable diffs
    # regardless of how the source wrapped its parameters.
    stripped = (
        ast.AsyncFunctionDef
        if isinstance(func_node, ast.AsyncFunctionDef)
        else ast.FunctionDef
    )
    # Defaults execute even with postponed annotations. Their presence belongs
    # to the protocol, but evaluating implementation factories, sentinels or
    # enum members here would introduce a second runtime dependency authority.
    # Ellipsis preserves positional/keyword requiredness without those imports.
    args = copy.deepcopy(func_node.args)
    args.defaults = [ast.Constant(value=Ellipsis) for _ in args.defaults]
    args.kw_defaults = [
        None if default is None else ast.Constant(value=Ellipsis)
        for default in args.kw_defaults
    ]
    rebuilt = stripped(
        name=func_node.name,
        args=args,
        body=[ast.Expr(value=ast.Constant(value=Ellipsis))],
        decorator_list=[],
        returns=copy.deepcopy(func_node.returns),
        type_comment=None,
        type_params=copy.deepcopy(getattr(func_node, "type_params", [])),
    )
    ast.fix_missing_locations(rebuilt)
    header = ast.unparse(rebuilt)
    # ``ast.unparse`` emits "def f(...):\n    ...". Collapse the trailing body
    # onto the signature so generated diffs stay dense and reviewable.
    if header.endswith("\n    ..."):
        header = header[: -len("\n    ...")] + " ..."
    indented = textwrap.indent(header, "    ")
    return "\n".join([*(f"    @{name}" for name in method.decorators), indented])


# ---------------------------------------------------------------------------
# Attribute surface + curated type table
# ---------------------------------------------------------------------------


def _method_body(method: ast.FunctionDef | ast.AsyncFunctionDef) -> MethodBody:
    """Collect stores and explicit annotations together from one root method.

    Nested helper classes/functions are not generator state: several lowering
    methods define local visitors whose ``self.x`` stores belong to the helper
    object, not ``SimpleTIRGenerator``. Count only stores to the root method's
    ``self`` parameter and do not descend into nested scopes.
    """
    positional = [*method.args.posonlyargs, *method.args.args]
    if not positional or positional[0].arg != "self":
        return MethodBody(frozenset(), ())

    attrs: set[str] = set()
    annotations: dict[str, str] = {}

    class Visitor(ast.NodeVisitor):
        def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
            if node is method:
                self.generic_visit(node)

        def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
            if node is method:
                self.generic_visit(node)

        def visit_ClassDef(self, node: ast.ClassDef) -> None:
            return

        def visit_Lambda(self, node: ast.Lambda) -> None:
            return

        def visit_Attribute(self, node: ast.Attribute) -> None:
            if (
                isinstance(node.value, ast.Name)
                and node.value.id == "self"
                and isinstance(node.ctx, ast.Store)
            ):
                attrs.add(node.attr)
            self.generic_visit(node)

        def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
            target = node.target
            if (
                isinstance(target, ast.Attribute)
                and isinstance(target.value, ast.Name)
                and target.value.id == "self"
            ):
                annotations.setdefault(target.attr, ast.unparse(node.annotation))
            self.generic_visit(node)

    Visitor().visit(method)
    return MethodBody(frozenset(attrs), tuple(annotations.items()))


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


def _collect_surface(
    surface_classes: list[type],
    builtins: set[str],
    *,
    curated: dict[str, str],
    source_index: PythonSourceIndex,
) -> ProtocolSurface:
    """Resolve the assembled MRO once and derive every protocol fact from it.

    Signatures use the most-derived binding. Attribute coverage still includes
    every contributing implementation, including overridden base methods. The
    source index and body facts live only for this inspection/generation.
    """
    chosen: dict[str, MethodSurface] = {}
    bodies: dict[object, MethodBody] = {}
    names: set[str] = set()
    class_table: dict[str, str] = {}
    store_table: dict[str, str] = {}
    unresolved: list[str] = []
    for klass in surface_classes:
        for name, annotation in getattr(klass, "__annotations__", {}).items():
            names.add(name)
            class_table.setdefault(
                name,
                annotation
                if isinstance(annotation, str)
                else _annotation_to_text(annotation),
            )
        for name, value in vars(klass).items():
            selected = (
                (not name.startswith("__") or name == "__init__")
                and (
                    klass is not ast.NodeVisitor
                    or name in _NODE_VISITOR_DISPATCH_METHODS
                )
                and (name not in builtins or name == "__init__")
                and (callable(value) or isinstance(value, (staticmethod, classmethod)))
                and name not in chosen
            )
            try:
                source = source_index.function(value)
            except PythonSourceError as error:
                unresolved.append(f"{klass.__qualname__}.{name}: {error}")
                continue
            if source is None:
                if selected:
                    unresolved.append(
                        f"{klass.__qualname__}.{name}: no Python function source"
                    )
                continue
            body = bodies.get(source.function)
            if body is None:
                body = _method_body(source.node)
                bodies[source.function] = body
            names.update(body.attrs)
            for attr, annotation in body.annotations:
                store_table.setdefault(attr, annotation)
            if selected:
                chosen[name] = _method_surface(name, value, source, body)
    if unresolved:
        raise ProtocolGenError(
            "could not extract the real method/attribute surface: "
            + "; ".join(unresolved)
        )
    attrs = tuple(
        (
            name,
            class_table.get(name)
            or store_table.get(name)
            or curated.get(name)
            or "Any",
        )
        for name in sorted(names - builtins)
    )
    return ProtocolSurface(tuple(chosen[name] for name in sorted(chosen)), attrs)


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

    for text in dict.fromkeys(annotation_texts):
        visit_expr(text)
    return found


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
    methods: tuple[MethodSurface, ...],
    *,
    types_module_exports: set[str],
) -> str:
    annotation_texts = [a for _n, a in attrs_second_half]
    annotation_texts.extend(
        text for method in methods for text in method.annotation_texts
    )
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
    context_decorators = sorted(
        {
            decorator
            for method in methods
            for decorator in method.decorators
            if decorator in {"contextmanager", "asynccontextmanager"}
        }
    )
    if context_decorators:
        extra.append(f"from contextlib import {', '.join(context_decorators)}")
    imports = _render_import_block(
        typing_names, types_names, tc_lines, needs_ast, extra_imports=extra
    )
    body_parts: list[str] = [
        "\nclass _GeneratorProtocol(_GeneratorProtocolAttrs, Protocol):\n"
    ]
    if attrs_second_half:
        body_parts.append(_render_attrs_block(attrs_second_half))
        body_parts.append("\n")
    for method in methods:
        body_parts.append(_render_method_stub(method))
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


def _format_generated_text(path: Path, text: str) -> str:
    """Format generated Python text through the repository formatter."""
    cmd = [
        sys.executable,
        "-m",
        "ruff",
        "format",
        "--no-force-exclude",
        "--stdin-filename",
        str(path),
        "-",
    ]
    proc = subprocess.run(
        cmd,
        input=text,
        text=True,
        capture_output=True,
        cwd=ROOT,
        check=False,
    )
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout).strip()
        raise ProtocolGenError(
            f"failed to format generated protocol text for {path.relative_to(ROOT)}: "
            f"{detail}"
        )
    return proc.stdout


def generate() -> dict[Path, str]:
    """Render both generated files. Returns ``{path: rendered_text}``."""
    import molt.frontend._types as types_module

    types_module_exports = {n for n in dir(types_module) if not n.startswith("__")}

    generator = _load_generator()
    surface_classes = _surface_classes(generator)
    builtins = _builtin_names()

    surface = _collect_surface(
        surface_classes,
        builtins,
        curated=_harvest_curated_attr_types([OUT_ATTRS, OUT_PROTOCOL]),
        source_index=PythonSourceIndex(),
    )
    attrs_first, attrs_second = _split_attrs(list(surface.attrs))

    attrs_text = render_attrs_file(
        attrs_first, types_module_exports=types_module_exports
    )
    protocol_text = render_protocol_file(
        attrs_second, surface.methods, types_module_exports=types_module_exports
    )
    return {
        OUT_ATTRS: _format_generated_text(OUT_ATTRS, attrs_text),
        OUT_PROTOCOL: _format_generated_text(OUT_PROTOCOL, protocol_text),
    }


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def _check(path: Path, rendered: str) -> bool:
    """Return True if *path* is in sync with *rendered* (prints a hint if not)."""
    if not path.exists():
        print(f"MISSING generated file: {path}", file=sys.stderr)
        return False
    if not generated_file_matches(path, rendered):
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
        write_generated_text(path, text)
        print(f"wrote {path.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
