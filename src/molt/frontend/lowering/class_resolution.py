"""ClassResolutionMixin: typed class identity, MRO, and method lookup (HF-203).

One authority for class identity in the frontend. Every class statement gets a
:class:`ClassId` before lowering starts; class facts are keyed by it; a source
read resolves to it through binding flow; an annotation resolves to it through
the lexical class bindings of its scope. Nothing here compares class names.
"""

from __future__ import annotations

import ast
from collections import Counter
from dataclasses import dataclass, field
from typing import Literal

from molt.compiler_analysis.python_lexical_scope import (
    function_annotation_expressions,
    function_parameter_names,
    python_scope_declarations,
)
from molt.compiler_analysis.python_identity_catalog_generated import (
    PythonIdentity,
)
from molt.compiler_analysis.python_source_keys import (
    PythonSourceKey,
    python_node_source_key,
)
from molt.frontend._mixin_base import GeneratorMixinBase
from molt.frontend._types import (
    _BUILTIN_FAST_METHODS,
    BUILTIN_TYPE_TAGS,
    BoundMethodHint,
    BuiltinClass,
    ClassId,
    ClassRef,
    DeclaredClass,
    MethodInfo,
    MoltOp,
    MoltValue,
    ReceiverKind,
    TypeHint,
    layout_definition_symbol,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.python_private_names import python_definition_name


@dataclass(frozen=True, slots=True)
class ModuleImportBinding:
    """An annotation names a module binding that only ``from`` imports write."""

    name: str


type AnnotationClassBinding = ClassId | ModuleImportBinding

# A read whose identities lack this bit is not a class object (it may be an
# instance the class created, which shares the class statement's origin).
_USER_CLASS_IDENTITY = int(PythonIdentity.USER_CLASS)

# Hint spellings the backend reads as something other than a user class.
_NON_CLASS_HINT_SPELLINGS = frozenset({"Any", "Unknown", "None"})


@dataclass(frozen=True, slots=True)
class ClassDefinitionIndex:
    """The module's class statements, computed once before lowering."""

    # ClassDef source key -> its identity.
    definitions: dict[PythonSourceKey, ClassId]
    # Class statements written directly in the module body, in source order.
    top_level: tuple[ClassId, ...]
    # Decorated and ``slots=True`` dataclass statements: their binding holds
    # the decorator's result, not the class the statement creates.
    excluded: frozenset[ClassId]
    # The namespace binding each statement writes (``ClassDef.name``).
    binding_names: dict[ClassId, str]
    # Annotation source key -> the class binding its name resolves to.
    annotations: dict[PythonSourceKey, AnnotationClassBinding]


@dataclass(slots=True)
class _LexicalFrame:
    kind: Literal["module", "function", "class"]
    qualname: str
    bound: frozenset[str]
    globals: frozenset[str]
    nonlocals: frozenset[str]
    classes: dict[str, list[ast.ClassDef]] = field(default_factory=dict)
    imports: set[str] = field(default_factory=set)


def _child_qualname(
    frame: _LexicalFrame,
    node: ast.ClassDef | ast.FunctionDef | ast.AsyncFunctionDef,
) -> str:
    """CPython's ``__qualname__`` rule, including the ``global`` reset."""
    name = python_definition_name(node)
    if frame.kind == "module" or node.name in frame.globals:
        return name
    if frame.kind == "function":
        return f"{frame.qualname}.<locals>.{name}"
    return f"{frame.qualname}.{name}"


def _annotation_binding_name(node: ast.expr) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        try:
            parsed = ast.parse(node.value.strip(), mode="eval").body
        except SyntaxError:
            return None
        if isinstance(parsed, ast.Name):
            return parsed.id
    return None


def _lexical_owner(name: str, chain: tuple[_LexicalFrame, ...]) -> _LexicalFrame | None:
    """The scope whose binding a name evaluated in ``chain[-1]`` reads."""
    innermost = len(chain) - 1
    for position in range(innermost, -1, -1):
        frame = chain[position]
        if frame.kind == "class" and position != innermost:
            # Class scopes are invisible to the scopes nested inside them.
            continue
        if frame.kind == "module":
            return frame if name in frame.bound else None
        if name in frame.globals:
            module = chain[0]
            return module if name in module.bound else None
        if name in frame.nonlocals:
            continue
        if name in frame.bound:
            return frame
    return None


class ClassResolutionMixin(GeneratorMixinBase):
    # ── Identity ────────────────────────────────────────────────────────

    def _build_class_definition_index(self, module: ast.Module) -> ClassDefinitionIndex:
        """Assign every class statement its ClassId and bind every annotation.

        ``occurrence`` is decided over the whole module before lowering, so the
        first and the second ``class P`` get consistent identities.
        """
        eager = self.eager_annotations and not self.future_annotations
        records: list[tuple[ast.ClassDef, str]] = []
        top_level: list[ast.ClassDef] = []
        annotation_refs: list[tuple[ast.expr, tuple[_LexicalFrame, ...]]] = []

        def frame_for(
            kind: Literal["module", "function", "class"],
            qualname: str,
            body: list[ast.stmt],
            parameters: tuple[str, ...] = (),
        ) -> _LexicalFrame:
            declarations = python_scope_declarations(
                body, parameters, eager_annotations=eager
            )
            return _LexicalFrame(
                kind,
                qualname,
                declarations.bound,
                declarations.globals,
                declarations.nonlocals,
            )

        def walk(statements: list[ast.stmt], chain: tuple[_LexicalFrame, ...]) -> None:
            frame = chain[-1]
            for statement in statements:
                if isinstance(statement, ast.ClassDef):
                    frame.classes.setdefault(statement.name, []).append(statement)
                    qualname = _child_qualname(frame, statement)
                    records.append((statement, qualname))
                    child = frame_for("class", qualname, statement.body)
                    walk(statement.body, (*chain, child))
                    continue
                if isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    for annotation in function_annotation_expressions(statement):
                        annotation_refs.append((annotation, chain))
                    child = frame_for(
                        "function",
                        _child_qualname(frame, statement),
                        statement.body,
                        function_parameter_names(statement.args),
                    )
                    walk(statement.body, (*chain, child))
                    continue
                if isinstance(statement, ast.AnnAssign):
                    annotation_refs.append((statement.annotation, chain))
                elif isinstance(statement, ast.ImportFrom):
                    frame.imports.update(
                        alias.asname or alias.name
                        for alias in statement.names
                        if alias.name != "*"
                    )
                for nested in ("body", "orelse", "finalbody"):
                    block = getattr(statement, nested, None)
                    if isinstance(block, list):
                        walk(block, chain)
                for handler in getattr(statement, "handlers", ()):
                    walk(handler.body, chain)
                if isinstance(statement, ast.Match):
                    for case in statement.cases:
                        walk(case.body, chain)

        module_frame = frame_for("module", "", module.body)
        walk(module.body, (module_frame,))
        top_level.extend(
            statement
            for statement in module.body
            if isinstance(statement, ast.ClassDef)
        )

        counts = Counter(qualname for _, qualname in records)
        definitions: dict[PythonSourceKey, ClassId] = {}
        binding_names: dict[ClassId, str] = {}
        excluded: set[ClassId] = set()
        for node, qualname in records:
            class_id = ClassId(
                self.module_name,
                qualname,
                (node.lineno, node.col_offset) if counts[qualname] > 1 else None,
            )
            definitions[python_node_source_key(node)] = class_id
            binding_names[class_id] = node.name
            try:
                dataclass_opts, other_decorators = self._class_decorator_plan(node)
            except FrontendRejection:
                # Lowering rejects the statement itself; it never binds a class.
                excluded.add(class_id)
                continue
            if other_decorators or (dataclass_opts or {}).get("slots"):
                excluded.add(class_id)

        annotations: dict[PythonSourceKey, AnnotationClassBinding] = {}
        for annotation, chain in annotation_refs:
            name = _annotation_binding_name(annotation)
            if name is None:
                continue
            owner = _lexical_owner(name, chain)
            if owner is None:
                continue
            statements = owner.classes.get(name, [])
            if len(statements) == 1:
                annotations[python_node_source_key(annotation)] = definitions[
                    python_node_source_key(statements[0])
                ]
            elif not statements and owner.kind == "module" and name in owner.imports:
                annotations[python_node_source_key(annotation)] = ModuleImportBinding(
                    name
                )
        return ClassDefinitionIndex(
            definitions=definitions,
            top_level=tuple(
                definitions[python_node_source_key(node)] for node in top_level
            ),
            excluded=frozenset(excluded),
            binding_names=binding_names,
            annotations=annotations,
        )

    def _class_id_for_definition(self, node: ast.ClassDef) -> ClassId:
        index = self._class_index
        class_id = (
            index.definitions.get(python_node_source_key(node))
            if index is not None
            else None
        )
        if class_id is None:
            raise FrontendRejection(
                Diagnostic.INTERNAL_INVARIANT,
                f"class statement {node.name!r} has no ClassId",
            )
        return class_id

    def _build_class_exports(self) -> dict[tuple[str, str], ClassId]:
        """``(module, binding) -> ClassId`` for other modules' unique classes.

        A module binding written by one class statement exports that class. A
        binding that several class statements write exports nothing.
        """
        candidates: dict[tuple[str, str], list[ClassId]] = {}
        for class_id, info in self.known_classes.items():
            if class_id.module == self.module_name or not info.get("static"):
                continue
            key = (class_id.module, info["binding_name"])
            candidates.setdefault(key, []).append(class_id)
        return {
            key: class_ids[0]
            for key, class_ids in candidates.items()
            if len(class_ids) == 1 and self._class_binding_is_the_class(class_ids[0])
        }

    def _class_binding_is_the_class(self, class_id: ClassId) -> bool:
        """False when the statement binds a decorator's result or a rebuilt class."""
        if class_id.module == self.module_name:
            index = self._class_index
            return index is not None and class_id not in index.excluded
        info = self.known_classes.get(class_id)
        if info is None or info.get("decorated"):
            return False
        return not (
            info.get("dataclass")
            and info.get("dataclass_params", {}).get("slots", False)
        )

    def _imported_binding_class(self, name: str) -> ClassId | None:
        """The exported class an import binding names, through the import tables."""
        module_name = self.imported_names.get(name)
        if (
            module_name is None
            and name not in self.locals
            and name not in self.boxed_locals
        ):
            module_name = self.global_imported_names.get(name)
        if module_name is None:
            return None
        return self._class_exports.get((module_name, self._imported_attr_name(name)))

    def _class_reference(self, node: ast.expr) -> ClassId | None:
        """The one class statement whose class object a source read evaluates to.

        Binding flow decides which statement: the read's ``source_origins`` must
        name exactly one definition, and its identities must admit a user class
        (an instance the class created carries the same origin but not that
        identity). The read must be of that statement's own binding, so an
        alias never inherits the class's binding custody. Anything else
        resolves to nothing, and the consumer takes the generic path. An import
        binding resolves through the exporting module's binding table.
        """
        if not isinstance(node, ast.Name):
            return None
        binding_index = self.python_binding_index
        index = self._class_index
        if binding_index is None or index is None:
            return None
        fact = binding_index.expression_fact(node)
        if fact is None or fact.binding_invalidated:
            return None
        origins = fact.result.source_origins
        if len(origins) != 1:
            return None
        (origin,) = origins
        class_id = index.definitions.get(origin)
        if class_id is not None:
            if (
                not fact.result.identities & _USER_CLASS_IDENTITY
                or class_id in index.excluded
                or index.binding_names[class_id] != node.id
            ):
                return None
            return class_id
        if origin[4] == "alias":
            return self._imported_binding_class(node.id)
        return None

    def _class_mutation_targets(self, node: ast.expr) -> frozenset[ClassId]:
        """Every class statement a mutated receiver may be (conservative)."""
        if not isinstance(node, ast.Name):
            return frozenset()
        binding_index = self.python_binding_index
        index = self._class_index
        if binding_index is None or index is None:
            return frozenset()
        fact = binding_index.expression_fact(node)
        if fact is None or not fact.result.identities & _USER_CLASS_IDENTITY:
            return frozenset()
        return frozenset(
            class_id
            for origin in fact.result.source_origins
            if (class_id := index.definitions.get(origin)) is not None
        )

    def _name_has_source_binding(self, node: ast.Name) -> bool:
        """True when binding flow shows a source binding can reach the read."""
        binding_index = self.python_binding_index
        fact = (
            binding_index.expression_fact(node) if binding_index is not None else None
        )
        return fact is not None and bool(fact.result.source_origins)

    def _base_class_reference(self, node: ast.expr) -> ClassRef | None:
        """A class statement, or a builtin class that no source binding shadows."""
        class_id = self._class_reference(node)
        if class_id is not None:
            return class_id
        if not isinstance(node, ast.Name) or self._name_has_source_binding(node):
            return None
        if node.id in BUILTIN_TYPE_TAGS or self._builtin_exception_is_available(
            node.id
        ):
            return BuiltinClass(node.id)
        return None

    def _annotation_class(self, node: ast.expr | None) -> ClassId | None:
        """The class an annotation names through its scope's class bindings."""
        index = self._class_index
        if node is None or index is None:
            return None
        binding = index.annotations.get(python_node_source_key(node))
        if isinstance(binding, ClassId):
            return None if binding in index.excluded else binding
        if isinstance(binding, ModuleImportBinding):
            module_name = self.global_imported_names.get(binding.name)
            if module_name is None:
                return None
            return self._class_exports.get(
                (module_name, self._imported_attr_name(binding.name))
            )
        return None

    # ── Layout and guards ───────────────────────────────────────────────

    def _class_layout_stable(self, class_id: ClassId) -> bool:
        class_info = self.classes.get(class_id)
        if not class_info:
            return False
        if class_info.get("dynamic") or class_info.get("dataclass"):
            return False
        if class_id in self.mutated_classes:
            return False
        return True

    def _class_layout_definition(self, class_id: ClassId) -> str | None:
        """The class statement a layout guard against ``class_id`` trusts.

        ``guard_layout_match`` admits a receiver whose class carries this
        statement's identity at the statement's layout version (HF-220). The
        identity is stamped when the statement runs, never read from a
        binding, so rebinding the name in this module or another cannot
        redirect a guard, and each run of the statement creates classes with
        the same facts. ``None``: no statement stamps the class, so the
        consumer emits the generic path.
        """
        class_info = self.classes.get(class_id)
        if class_info is None:
            return None
        return layout_definition_symbol(class_id, class_info)

    def _current_module_static_class_ref(self, class_id: ClassId) -> MoltValue | None:
        if self.current_func_name != "molt_main":
            return None
        if self.module_globals_dict_escaped:
            return None
        if class_id.module != self.module_name:
            return None
        if class_id in self.class_definition_pending:
            return None
        class_info = self.classes.get(class_id)
        if class_info is None:
            return None
        binding_name = class_info["binding_name"]
        if binding_name in self.module_global_mutations:
            return None
        if class_info.get("decorated"):
            return None
        if not self._class_layout_stable(class_id):
            return None
        static_name = class_info.get("class_value_name")
        if not static_name:
            return None
        current = self.globals.get(binding_name)
        if current is None or current.name != static_name:
            return None
        return current

    def _emit_class_method_func(
        self, class_obj: MoltValue, method_name: str
    ) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[class_obj, method_name],
                result=res,
            )
        )
        return res

    # ── Class graph ─────────────────────────────────────────────────────

    def _class_mro(self, class_ref: ClassRef) -> tuple[ClassRef, ...]:
        """The MRO the class statement recorded; a builtin or unknown class is
        its own one-entry MRO."""
        if isinstance(class_ref, BuiltinClass):
            return (class_ref,)
        info = self.classes.get(class_ref)
        if info is None:
            return (class_ref,)
        return info.get("mro", (class_ref,))

    def _class_is_exception_subclass(self, class_id: ClassId) -> bool:
        cached = self._exception_subclass_memo.get(class_id)
        if cached is not None:
            return cached
        result = False
        for base in self._class_mro(class_id)[1:]:
            if isinstance(base, BuiltinClass):
                if self._builtin_exception_is_available(base.name):
                    result = True
                    break
                continue
            if base in self.classes and self._class_is_exception_subclass(base):
                result = True
                break
        self._exception_subclass_memo[class_id] = result
        return result

    def _resolve_method_info(
        self, class_id: ClassId, method: str
    ) -> tuple[MethodInfo | None, ClassId | None]:
        if class_id in self.class_definition_pending:
            return None, None
        for owner in self._class_mro(class_id):
            if isinstance(owner, BuiltinClass):
                continue
            info = self.classes.get(owner)
            if not info:
                continue
            methods = info.get("methods", {})
            class_attrs = info.get("class_attrs", {})
            pending = info.get("pending_methods")
            # Avoid early binding to base methods when the current class
            # defines the method later in the class body.
            if pending and method in pending and method not in methods:
                return None, owner
            # Avoid binding to base methods when a class-level assignment
            # overrides the attribute with a non-method value.
            if method in class_attrs and method not in methods:
                return None, owner
            if method in methods:
                return methods[method], owner
        return None, None

    # ── Declared classes ────────────────────────────────────────────────

    @staticmethod
    def _declared_class(value: MoltValue | None) -> DeclaredClass | None:
        if value is None:
            return None
        hint = value.type_hint
        return hint if isinstance(hint, DeclaredClass) else None

    def _declared_instance_class(self, value: MoltValue | None) -> ClassId | None:
        """A declared instance's class, if its facts are known here."""
        declared = self._declared_class(value)
        if declared is None or declared.kind is not ReceiverKind.INSTANCE:
            return None
        return declared.class_id if declared.class_id in self.classes else None

    # ── Wire projection ─────────────────────────────────────────────────

    @staticmethod
    def _wire_type_hint(hint: TypeHint) -> str | None:
        """The SimpleIR ``type_hint`` spelling of a value hint, or None.

        A user class travels as its ``__name__``, the backend's opaque
        ``UserClass`` tag. A user class spelled like a builtin type would read
        as that builtin, so it travels as no hint at all.
        """
        if isinstance(hint, str):
            return hint
        if isinstance(hint, BoundMethodHint):
            return ClassResolutionMixin._bound_method_wire(hint)
        name = hint.class_id.name
        if name in BUILTIN_TYPE_TAGS or name in _NON_CLASS_HINT_SPELLINGS:
            return None
        return name

    @staticmethod
    def _bound_method_wire(hint: BoundMethodHint) -> str | None:
        """``BoundMethod:<owner>:<method>`` for the backend's builtin fast paths.

        A user class whose name matches a builtin fast-path owner travels as no
        hint, so the backend never applies a builtin method to it.
        """
        owner = hint.owner
        if isinstance(owner, ClassId) and owner.name in _BUILTIN_FAST_METHODS:
            return None
        return f"BoundMethod:{owner.name}:{hint.method}"
