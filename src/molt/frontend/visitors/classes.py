"""ClassDefVisitorMixin: class-definition lowering (F1 decomposition).

Move-only extraction from frontend/__init__.py (F1 phase 2). Covers
visit_ClassDef and its exclusively-owned class/MRO/descriptor/inline-init and
method-closure helpers (every method here is, transitively, called only from
within this family). self.<method> / self.<attr> references resolve through the
SimpleTIRGenerator MRO at runtime.
"""

from __future__ import annotations

import ast
from molt.python_private_names import python_definition_name
from typing import (
    Any,
    Callable,
    cast,
)

from molt.compiler_analysis.python_identity_catalog_generated import PythonIdentity
from molt.compiler_analysis.python_source_keys import python_node_source_key
from molt.compiler_analysis.python_lexical_scope import (
    PythonLexicalScopeVisitor,
    DefinitionLexicalRegions,
    LexicalDefinitionNode,
    class_body_functions,
    definition_lexical_regions,
)
from molt.frontend._types import (
    BUILTIN_LAYOUT_MIN,
    BUILTIN_TYPE_TAGS,
    OBJECT_CLASS,
    BuiltinClass,
    ClassId,
    ClassInfo,
    ClassRef,
    MethodInfo,
    MoltOp,
    MoltValue,
    TypeHint,
    _ClassNsScope,
    _function_is_instance_method,
    layout_definition_symbol,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend.sema import (
    c3_merge,
)
from molt.frontend.visitors.class_method_compilation import (
    ClassMethodCompilationMixin,
)


class ClassDefVisitorMixin(ClassMethodCompilationMixin):
    def _class_layout_version(
        self,
        class_id: ClassId,
        class_attrs: dict[str, ast.expr],
        methods: dict[str, MethodInfo] | None = None,
        method_count: int | None = None,
    ) -> int:
        class_info = self.classes[class_id]
        field_offsets = (
            1
            if class_info.get("fields")
            and not class_info.get("dynamic")
            and not class_info.get("dataclass")
            else 0
        )
        if method_count is None:
            method_count = len(methods or {})
        return 1 + field_offsets + len(class_attrs) + method_count

    def _class_constructor_fold_safe(
        self, class_id: ClassId, class_info: ClassInfo
    ) -> bool:
        if class_id.module != self.module_name:
            return False
        if not class_info.get("constructor_binding_proven"):
            return False
        index = self._class_index
        if index is None or class_id not in index.top_level:
            return False
        binding_name = class_info["binding_name"]
        if binding_name not in self.stable_module_classes:
            return False
        if self.module_globals_dict_escaped:
            return False
        if binding_name in self.module_global_mutations:
            return False
        if (
            class_info.get("dynamic")
            or class_info.get("dataclass")
            or class_info.get("custom_metaclass")
            or class_info.get("decorated")
        ):
            return False
        # A class that defines `__del__` (directly or anywhere in its MRO except
        # `object`) has a finalizer that CPython runs at the LAST reference drop.
        # The constructor fold inlines `__init__` and statically tracks
        # `self.attr = value`, so a later `obj.attr` read is replaced by the
        # tracked constant — which ERASES the object's last SSA use. The drop
        # pass then releases the instance right after its `__init__` field store,
        # firing `__del__` far earlier than Python's scope-visible drop (and in
        # the wrong order across multiple instances). Stack promotion (→ IMMORTAL)
        # and RC-strip in the escape pass compound this. None of those
        # optimizations is sound for a finalizer-bearing instance, so decline the
        # fold entirely: route `Demo()` through the normal `type.__call__` path,
        # where `obj.attr` stays a real load and the drop lands at the Python
        # scope boundary. `__del__` classes are rare and inherently slow, so the
        # lost fold is the correct trade for finalizer-dispatch parity.
        if self._class_defines_finalizer(class_id):
            return False
        # `object_new_bound` and the inlined-init constructor fold are only
        # equivalent to `type.__call__` when the MRO resolves `__new__` to
        # default `object.__new__`.  Custom or opaque `__new__` must stay on the
        # runtime class-call route so inherited overrides consume constructor
        # args and decide whether `__init__` should run.
        if not self._class_resolves_default_object_new(class_id, class_info):
            return False
        for owner in self._class_mro(class_id):
            info = self.classes.get(owner) if isinstance(owner, ClassId) else None
            if info is None:
                continue
            initializer = info.get("methods", {}).get("__init__")
            if initializer is not None:
                # The existing body proof admits only field assignments and
                # implicit/explicit None. Other initializers require the real
                # type protocol's return-value validation and call context.
                if initializer.get("inline_init_assigns") is None:
                    return False
                break
        return self._class_layout_stable(class_id)

    def _class_defines_finalizer(self, class_id: ClassId) -> bool:
        """True iff ``class_id`` resolves a user-defined ``__del__`` through its
        MRO (excluding ``object``). Used to suppress lifetime-shortening
        optimizations that would skip or mis-time finalizer dispatch.

        Resolves directly over each MRO class's ``methods`` table rather than via
        ``_resolve_method_info``: the constructor-fold decision is taken while the
        class is still in ``class_definition_pending`` (its body is processed but
        the registration is not finalized), and ``_resolve_method_info``
        short-circuits to ``(None, None)`` for a pending class — which would hide a
        ``__del__`` the class plainly defines. The per-class ``methods`` dict is
        already fully populated at this point, so a direct MRO walk is the sound
        source of truth here. A class-level assignment that shadows ``__del__``
        with a non-method value (present in ``class_attrs`` but not ``methods``)
        does not install a finalizer, matching ``_resolve_method_info``'s
        override rule."""
        for owner in self._class_mro(class_id):
            if isinstance(owner, BuiltinClass):
                continue
            info = self.classes.get(owner)
            if not info:
                continue
            methods = info.get("methods", {})
            if "__del__" in methods:
                return True
            class_attrs = info.get("class_attrs", {})
            if "__del__" in class_attrs:
                # A non-method override at this level masks any base __del__.
                return False
        return False

    def _builtin_min_layout(self, mro: tuple[ClassRef, ...]) -> int:
        min_size = 0
        for entry in mro:
            if isinstance(entry, BuiltinClass):
                min_size = max(min_size, BUILTIN_LAYOUT_MIN.get(entry.name, 0))
        return min_size

    def _class_reserved_tail_size(self, mro: tuple[ClassRef, ...]) -> int:
        return 16 if BuiltinClass("dict") in mro else 8

    def _setup_class_annotations(
        self, node: ast.ClassDef, scope: _ClassNsScope
    ) -> None:
        """Establish annotation storage before any conditional body execution."""

        if (
            self.python_binding_index is not None
            and self.python_binding_index.class_annotation_namespace_required(node)
        ):
            self._create_class_annotation_namespace(scope)

        class Collector(PythonLexicalScopeVisitor):
            found = False

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                if isinstance(node.target, ast.Name) and node.simple:
                    self.found = True

        collector = Collector(eager_annotations=self.eager_annotations)
        for statement in node.body:
            collector.visit(statement)
        if not collector.found:
            return
        if not (self.future_annotations or self.eager_annotations):
            # Execution marks are private compiler storage, not class attributes
            # or rebindable outer names. Every body path shares this dominating
            # map, and the evaluator captures the object directly.
            exec_map = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=exec_map))
            self.class_annotation_exec_map = exec_map
            return
        if scope.ns is None:
            annotations = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=annotations))
            self._class_ns_store(scope, "__annotations__", annotations)
            return
        key = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["__annotations__"], result=key))
        missing = self._emit_missing_value()
        existing = self._emit_runtime_call(
            "molt_namespace_get", [scope.ns, key, missing]
        )
        absent = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[existing, missing], result=absent))
        self.emit(MoltOp(kind="IF", args=[absent], result=MoltValue("none")))
        annotations = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=annotations))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[scope.ns, key, annotations],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_class_annotated_assignment(
        self, node: ast.AnnAssign, scope: _ClassNsScope
    ) -> None:
        """Publish the value before evaluating and publishing its annotation."""
        assert isinstance(node.target, ast.Name)
        if node.value is not None:
            value = self.visit(node.value)
            if value is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported class body assignment"
                )
            self._class_ns_store(scope, node.target.id, value)
            self.locals[node.target.id] = value
        if self.future_annotations or self.eager_annotations:
            value = self._emit_annotation_value(
                node.annotation, stringize=self.future_annotations
            )
            if not node.simple:
                return
            annotations = self._class_ns_load(scope, "__annotations__")
            if annotations is None:
                annotations = self._emit_global_get("__annotations__")
            key = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.target.id], result=key))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[annotations, key, value],
                    result=MoltValue("none"),
                )
            )
        elif node.simple:
            exec_map = self.class_annotation_exec_map
            if exec_map is None:
                raise FrontendRejection(
                    Diagnostic.INTERNAL_INVARIANT,
                    "Class annotation execution storage was not initialized at body entry",
                )
            exec_id = self._annotation_exec_id(is_module=False)
            self._emit_annotation_exec_mark(exec_map, exec_id)
            self.class_annotation_items.append(
                (node.target.id, node.annotation, exec_id)
            )

    def _create_class_annotation_namespace(self, scope: _ClassNsScope) -> None:
        """Capture the namespace owner, never the class's rebindable source name.

        The shared cell initially owns the live body mapping. Type construction
        replaces its contents with the copied class dict before user callbacks.
        The binding index requests this once at body entry, before any branch,
        loop or evaluator can capture it. Classes without such reads allocate
        neither a namespace cell nor an additional mapping.
        """
        if scope.annotation_namespace_cell is not None:
            raise FrontendRejection(
                Diagnostic.INTERNAL_INVARIANT,
                "Class annotation namespace already has a storage owner",
            )
        if scope.ns is None:
            items: list[MoltValue] = []
            for name, value in scope.attr_values.items():
                key = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key))
                items.extend([key, value])
            scope.ns = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=items, result=scope.ns))
        scope.annotation_namespace_cell = self._emit_cell_new(scope.ns)

    def _collect_static_attributes(self, class_node: ast.ClassDef) -> tuple[str, ...]:
        """CPython's syntactic self stores, independently of descriptor layout.

        Python 3.13 records the literal ``self`` spelling in function bodies,
        including nested functions and decorated methods, and sorts the result.
        It does not infer the first parameter's receiver. Only compiled stores
        count; annotation-only targets and stringized annotations produce none.
        """
        attrs: set[str] = set()
        eager = self.eager_annotations and not self.future_annotations
        future = self.future_annotations

        class SelfAttrCollector(ast.NodeVisitor):
            in_function = False

            def visit_function_nodes(self, nodes: tuple[ast.AST, ...]) -> None:
                previous = self.in_function
                self.in_function = True
                try:
                    for node in nodes:
                        self.visit(node)
                finally:
                    self.in_function = previous

            def visit_header(
                self, node: LexicalDefinitionNode
            ) -> DefinitionLexicalRegions:
                regions = definition_lexical_regions(
                    node, eager_annotations=eager, future_annotations=future
                )
                for expression in regions.enclosing:
                    self.visit(expression)
                self.visit_function_nodes(regions.annotations)
                return regions

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                self.visit_function_nodes(self.visit_header(node).body)

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                self.visit_function_nodes(self.visit_header(node).body)

            def visit_Lambda(self, node: ast.Lambda) -> None:
                self.visit_function_nodes(self.visit_header(node).body)

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
                self.visit_function_nodes(self.visit_header(node).body)

            def visit_TypeAlias(self, node: ast.TypeAlias) -> None:
                self.visit_header(node)

            def visit_Attribute(self, node: ast.Attribute) -> None:
                if (
                    self.in_function
                    and isinstance(node.ctx, ast.Store)
                    and isinstance(node.value, ast.Name)
                    and node.value.id == "self"
                ):
                    attrs.add(node.attr)
                self.generic_visit(node)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                if node.value is not None:
                    self.visit(node.value)
                    self.visit(node.target)
                elif isinstance(node.target, ast.Attribute):
                    self.visit(node.target.value)
                elif isinstance(node.target, ast.Subscript):
                    self.visit(node.target.value)
                    self.visit(node.target.slice)
                if not self.in_function and not future:
                    if eager:
                        self.visit(node.annotation)
                    else:
                        self.visit_function_nodes((node.annotation,))

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                # Nested classes own their body metadata; their headers still
                # execute in the enclosing function.
                self.visit_header(node)

        collector = SelfAttrCollector()
        for statement in class_node.body:
            collector.visit(statement)
        return tuple(sorted(attrs))

    def _verify_classcell_result(
        self, class_name: str, cell: MoltValue, result: MoltValue
    ) -> None:
        """Verify the original cell after metaclass return, before decorators.

        The constructor alone fills cells. A metaclass may copy or consume its
        input mapping; only the original cell contents and returned type matter.
        Non-type results deliberately bypass this __build_class__ check.
        """
        type_value = self._emit_builtin_type_value("type")
        actual_type = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="TYPE_OF", args=[result], result=actual_type))
        is_type = self._emit_runtime_call(
            "molt_issubclass", [actual_type, type_value], type_hint="bool"
        )
        self.emit(MoltOp(kind="IF", args=[is_type], result=MoltValue("none")))
        owner = self._emit_cell_get(cell)
        correct = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[owner, result], result=correct))
        self.emit(MoltOp(kind="IF", args=[correct], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        missing = self._emit_missing_value()
        empty = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[owner, missing], result=empty))
        self.emit(MoltOp(kind="IF", args=[empty], result=MoltValue("none")))

        def message(parts: list[str | MoltValue]) -> MoltValue:
            values = []
            for part in parts:
                if isinstance(part, str):
                    value = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[part], result=value))
                else:
                    value = self._emit_repr_from_obj(part)
                values.append(value)
            return self._emit_string_join(values)

        missing_message = message(
            [
                f"__class__ not set defining {class_name!r} as ",
                result,
                ". Was __classcell__ propagated to type.__new__?",
            ]
        )
        exception = self._emit_exception_new("RuntimeError", missing_message)
        self._emit_raise(exception)
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        wrong_message = message(
            [
                "__class__ set to ",
                owner,
                f" defining {class_name!r} as ",
                result,
            ]
        )
        exception = self._emit_exception_new("TypeError", wrong_message)
        self._emit_raise(exception)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _class_decorator_plan(
        self, node: ast.ClassDef
    ) -> tuple[dict[str, bool] | None, list[ast.expr]]:
        """The recognized ``@dataclass`` options and every other decorator.

        One authority for lowering and for class-identity resolution: a class
        bound through another decorator, or rebuilt by ``slots=True``, is not
        the class its statement creates.
        """
        dataclass_opts: dict[str, bool] | None = None
        other_decorators: list[ast.expr] = []
        index = self.python_binding_index
        for position, deco in enumerate(node.decorator_list):
            target = deco.func if isinstance(deco, ast.Call) else deco
            if (
                index is None
                or not index.expression_is(target, PythonIdentity.DATACLASSES_DATACLASS)
                # The static layout describes the undecorated class. An inner
                # wrapper may return a different class; run that chain normally.
                or position != len(node.decorator_list) - 1
                or isinstance(deco, ast.Call)
                and deco.args
            ):
                other_decorators.append(deco)
                continue
            dataclass_opts = {
                "init": True,
                "repr": True,
                "eq": True,
                "order": False,
                "unsafe_hash": False,
                "frozen": False,
                "match_args": True,
                "kw_only": False,
                "slots": False,
                "weakref_slot": False,
            }
            if not isinstance(deco, ast.Call):
                continue
            for keyword in deco.keywords:
                if keyword.arg is None:
                    # A name-keyed constant table cannot prove the mapping's
                    # contents at this decorator's source point. Evaluate its
                    # real argument protocol through the ordinary runtime call.
                    dataclass_opts = None
                    break
                if keyword.arg not in dataclass_opts or not (
                    isinstance(keyword.value, ast.Constant)
                    and type(keyword.value.value) is bool
                ):
                    dataclass_opts = None
                    break
                dataclass_opts[keyword.arg] = keyword.value.value
            if dataclass_opts is None:
                other_decorators.append(deco)
        return dataclass_opts, other_decorators

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        previous_annotations = self.class_annotation_items
        previous_exec_map = self.class_annotation_exec_map
        previous_exec_counter = self.class_annotation_exec_counter
        previous_type_params = self.type_parameter_bindings
        try:
            self._emit_class_definition(node)
        finally:
            self.class_annotation_items = previous_annotations
            self.class_annotation_exec_map = previous_exec_map
            self.class_annotation_exec_counter = previous_exec_counter
            self.type_parameter_bindings = previous_type_params

    def _emit_class_definition(self, node: ast.ClassDef) -> None:
        class_id = self._class_id_for_definition(node)
        self.local_class_ids.append(class_id)
        # CPython 3.13 added __firstlineno__ and __static_attributes__ to every
        # class namespace; earlier targets never see either key.
        source_keys = self.target_python >= (3, 13)
        self.class_annotation_items = []
        self.class_annotation_exec_map = None
        self.class_annotation_exec_counter = 0
        dataclass_opts, other_decorators = self._class_decorator_plan(node)
        # Python captures every decorator expression before class execution,
        # then applies the captured values in reverse order. Static dataclass
        # metadata never authorizes a later lookup of the decorator binding.

        decorator_vals: list[MoltValue] = []
        if node.decorator_list:
            for deco in node.decorator_list:
                decorator_val = self.visit(deco)
                if decorator_val is None:
                    raise FrontendRejection(
                        Diagnostic.SYNTAX_FORM, "Unsupported class decorator"
                    )
                decorator_vals.append(self._capture_expression_reference(decorator_val))

        type_param_vals, type_param_map = self._emit_type_params_values(node)
        if type_param_map:
            merged = dict(self.type_parameter_bindings)
            merged.update(type_param_map)
            self.type_parameter_bindings = merged

        base_vals: list[MoltValue] = []
        # Each base resolves through binding flow to a class statement or a
        # builtin class; an unresolved base (``None``) builds dynamically.
        bases: list[ClassRef] = []
        base_resolution: list[ClassRef | None] = []
        has_explicit_bases = bool(node.bases)
        expanded_bases: MoltValue | None = None
        if any(isinstance(base, ast.Starred) for base in node.bases):
            # Class construction includes implicit body/name positional operands,
            # so even a sole *bases is materialized before keyword evaluation.
            # Tuple-display lowering already owns this source-ordered expansion.
            bases_expression = ast.copy_location(
                ast.Tuple(elts=node.bases, ctx=ast.Load()), node
            )
            prev_base_in_annotation = self.in_annotation
            if type_param_map:
                self.in_annotation = True
            try:
                expanded_bases = self.visit(bases_expression)
            finally:
                self.in_annotation = prev_base_in_annotation
            if expanded_bases is None:
                raise FrontendRejection(
                    Diagnostic.TYPE_FORM, "Unsupported expanded class bases"
                )
            base_resolution.append(None)
        elif node.bases:
            for base_expr in node.bases:
                prev_base_in_annotation = self.in_annotation
                if type_param_map:
                    self.in_annotation = True
                try:
                    base_val = self.visit(base_expr)
                finally:
                    self.in_annotation = prev_base_in_annotation
                if base_val is None:
                    raise FrontendRejection(
                        Diagnostic.TYPE_FORM,
                        "Base class must be defined before use",
                    )
                base_vals.append(base_val)
                base_ref = self._base_class_reference(base_expr)
                base_resolution.append(base_ref)
                if base_ref is not None:
                    bases.append(base_ref)

        has_metaclass_kw = False
        if node.keywords:
            for kw in node.keywords:
                if kw.arg == "metaclass":
                    has_metaclass_kw = True
                    break

        dynamic_build = False
        inherits_custom_meta = False
        if node.keywords:
            dynamic_build = True
        for base_ref in base_resolution:
            if base_ref is None:
                dynamic_build = True
                continue
            if isinstance(base_ref, BuiltinClass):
                continue
            base_info = self.classes.get(base_ref)
            if base_info is None:
                dynamic_build = True
                continue
            if base_info.get("custom_metaclass"):
                inherits_custom_meta = True
                dynamic_build = True
            if "__mro_entries__" in base_info.get("methods", {}):
                dynamic_build = True
        # A class body that contains anything beyond straight-line attribute
        # bindings / method-and-nested-class definitions (i.e. control flow —
        # for/if/while/try/with/match — or ``del``, or any augmented/looped
        # rebind of a class-scope name) must execute as a NORMAL BLOCK whose
        # mutable namespace is a real dict (CPython's class-body code object over
        # ``f_locals``).  Forcing ``dynamic_build`` gives that body a heap-backed
        # namespace mapping which is the loop-carried-correct store for its
        # names; the straight-line static fast path is untouched.  (P0 #50.)
        assert self._sema is not None, "module sema must be populated before lowering"
        body_needs_block = id(node) in self._sema.class_facts.block_exec_class_nodes
        if body_needs_block:
            dynamic_build = True
        if not has_explicit_bases:
            bases = [OBJECT_CLASS]

        if not base_vals and not dynamic_build:
            tag_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[BUILTIN_TYPE_TAGS["object"]], result=tag_val)
            )
            base_val = MoltValue(self.next_var(), type_hint="type")
            self.emit(MoltOp(kind="BUILTIN_TYPE", args=[tag_val], result=base_val))
            base_vals = [base_val]
            bases = [OBJECT_CLASS]

        methods: dict[str, MethodInfo] = {}
        method_nodes = class_body_functions(node)
        needs_classcell = self._lexical_dependencies().summary(node).class_cell_required
        class_attrs: dict[str, ast.expr] = {}
        class_attr_values: dict[str, MoltValue] = {}
        pending_methods = {item.name for item in method_nodes}
        # Repeated bases build dynamically like any multi-base class, and
        # molt_class_set_base raises CPython's TypeError when the statement
        # runs; base names cannot decide identity at compile time.
        dynamic = dynamic_build or len(bases) > 1
        opaque = (
            self.namespace_premise.opaque_activation_sources
            if self.namespace_premise is not None
            else None
        )
        source = python_node_source_key(node)
        definition_sources = (
            source,
            *(python_node_source_key(method) for method in class_body_functions(node)),
        )
        definition_exposed = opaque is not None and any(
            key in opaque for key in definition_sources
        )
        constructor_binding_proven = (
            self.namespace_premise is not None
            and self.namespace_premise.foreign_writes is not None
            and opaque is not None
            and not definition_exposed
        )
        if definition_exposed:
            dynamic = True
        for base_ref in bases:
            if isinstance(base_ref, BuiltinClass):
                continue
            base_info = self.classes.get(base_ref)
            if base_info is None or base_info.get("dynamic"):
                dynamic = True
        if class_id in self.mutated_classes:
            dynamic = True
        # ``static`` marks a module-level top-level class whose global binding
        # can supply optional identity for its methods' layout guards. A missing
        # or rebound binding must leave generic instance access available.
        # A *nested* ``class`` statement (``_class_body_depth > 0``) is
        # bound only into its enclosing class namespace — it has no module-global
        # name — so it must be treated exactly like a function-local class
        # (``current_func_name != "molt_main"``): non-static, routing its
        # methods' typed-field accesses through the instance-based generic path
        # rather than a non-existent module attribute.
        is_static = (
            self.current_func_name == "molt_main" and self._class_body_depth == 0
        )

        base_mros: list[list[ClassRef]] = [
            list(self._class_mro(base_ref)) for base_ref in bases
        ]
        base_mros.append(list(bases))
        merged = c3_merge(base_mros)
        if merged is None:
            merged = list(bases)
        mro: tuple[ClassRef, ...] = (class_id, *merged)
        # Native base classes own their payload prefix. Inferred Python fields
        # cannot certify offsets into that storage; runtime class sealing and
        # slot assignment own the layout, including transitive subclasses.
        if any(
            entry != OBJECT_CLASS
            and not (isinstance(entry, ClassId) and entry in self.classes)
            for entry in mro[1:]
        ):
            dynamic = True

        if dataclass_opts is not None:
            field_order: list[str] = []
            field_hints: dict[str, TypeHint] = {}
            for mro_entry in mro[1:]:
                base_info = (
                    self.classes.get(mro_entry)
                    if isinstance(mro_entry, ClassId)
                    else None
                )
                if base_info and base_info.get("dataclass"):
                    for name in base_info.get("field_order", []):
                        if name not in field_order:
                            field_order.append(name)

            init_requires_binding = any(
                isinstance(base, ClassId)
                and self.classes.get(base, {}).get(
                    "dataclass_init_requires_binding", False
                )
                for base in mro[1:]
            )
            fields_known = not self.future_annotations

            def _annotation_kind(annotation: ast.expr) -> str:
                index = self.python_binding_index
                if index is None or self.future_annotations:
                    return "unknown"
                target = (
                    annotation.value
                    if isinstance(annotation, ast.Subscript)
                    else annotation
                )
                for identity, kind in (
                    (PythonIdentity.DATACLASSES_KW_ONLY, "kw_only"),
                    (PythonIdentity.DATACLASSES_INITVAR, "initvar"),
                    (PythonIdentity.TYPING_CLASSVAR, "classvar"),
                ):
                    if index.expression_is(target, identity):
                        return kind
                fact = index.expression_fact(annotation)
                if (
                    fact is None
                    or isinstance(annotation, ast.Constant)
                    and isinstance(annotation.value, str)
                ):
                    return "unknown"
                # An unknown subscription or callback can return a dataclass
                # marker. Only a closed result identity proves a regular field.
                if fact.identities & int(PythonIdentity.OTHER | PythonIdentity.UNBOUND):
                    return "unknown"
                return "field"

            for item in node.body:
                if isinstance(item, ast.AnnAssign) and isinstance(
                    item.target, ast.Name
                ):
                    name = item.target.id
                    kind = _annotation_kind(item.annotation)
                    if kind in {"kw_only", "initvar"}:
                        init_requires_binding = True
                    if kind == "unknown":
                        fields_known = False
                    if kind == "kw_only":
                        continue
                    if kind not in {"classvar", "initvar"}:
                        if name not in field_order:
                            field_order.append(name)
                        if self._hints_enabled():
                            hint = self._annotation_to_hint(item.annotation)
                            if hint is not None:
                                field_hints[name] = hint
                    else:
                        if name in field_order:
                            field_order.remove(name)
                            field_hints.pop(name, None)
                    if item.value is not None:
                        class_attrs[name] = item.value
                if isinstance(item, ast.Assign):
                    for target in item.targets:
                        if isinstance(target, ast.Name):
                            class_attrs[target.id] = item.value
            if not fields_known:
                dynamic = True
                field_order = []
                field_hints = {}
            field_indices = {name: idx for idx, name in enumerate(field_order)}
            min_layout = self._builtin_min_layout(mro)
            size = max(len(field_order) * 8, min_layout)
            repr_generated = dataclass_opts["repr"] and "__repr__" not in methods
            self.classes[class_id] = {
                "binding_name": node.name,
                "fields": field_indices,
                "field_order": field_order,
                "field_hints": field_hints,
                "class_attrs": class_attrs,
                "possible_data_descriptor_attrs": self._possible_data_descriptor_attrs(
                    node, class_attrs
                ),
                "bases": tuple(bases),
                "mro": mro,
                "dynamic": dynamic,
                "static": is_static,
                "size": size,
                "dataclass": True,
                "frozen": dataclass_opts["frozen"],
                "repr": repr_generated,
                "slots": dataclass_opts["slots"],
                "dataclass_params": dataclass_opts,
                "dataclass_init_requires_binding": init_requires_binding,
                "constructor_binding_proven": constructor_binding_proven,
                "methods": methods,
                "pending_methods": pending_methods,
                "needs_classcell": needs_classcell,
                "custom_metaclass": has_metaclass_kw
                or inherits_custom_meta
                or dynamic_build,
                "decorated": bool(other_decorators),
            }
        else:
            fields: dict[str, int] = {}
            field_order: list[str] = []
            field_defaults: dict[str, ast.expr] = {}
            field_hints: dict[str, TypeHint] = {}
            for mro_entry in mro[1:]:
                base_info = (
                    self.classes.get(mro_entry)
                    if isinstance(mro_entry, ClassId)
                    else None
                )
                if base_info is None:
                    continue
                for field in base_info.get("field_order", []):
                    if field not in fields:
                        fields[field] = len(field_order) * 8
                        field_order.append(field)
                for field, hint in base_info.get("field_hints", {}).items():
                    if field not in field_hints:
                        field_hints[field] = hint
                for name, expr in base_info.get("defaults", {}).items():
                    if name not in field_defaults:
                        field_defaults[name] = expr

            def add_field(name: str) -> None:
                if name in fields:
                    return
                fields[name] = len(field_order) * 8
                field_order.append(name)

            def add_field_hint(name: str, annotation: ast.AST | None) -> None:
                if not self._hints_enabled() or annotation is None:
                    return
                hint = self._annotation_to_hint(cast(ast.expr, annotation))
                if hint is None or name in field_hints:
                    return
                field_hints[name] = hint

            for item in node.body:
                if isinstance(item, ast.AnnAssign) and isinstance(
                    item.target, ast.Name
                ):
                    add_field(item.target.id)
                    add_field_hint(item.target.id, item.annotation)
                    if item.value is not None:
                        field_defaults[item.target.id] = item.value
                        class_attrs[item.target.id] = item.value
                if isinstance(item, ast.Assign):
                    for target in item.targets:
                        if isinstance(target, ast.Name):
                            class_attrs[target.id] = item.value

            if "__slots__" in class_attrs:
                # Runtime declarations may be computed, repeated, or shadow an
                # ancestor slot. The runtime owns physical slot placement and
                # admission; a name-keyed inferred-field table cannot encode it.
                dynamic = True

            methods_in_body = [
                item for item in method_nodes if isinstance(item, ast.FunctionDef)
            ]
            if any(
                method.name
                in {
                    "__getattr__",
                    "__getattribute__",
                    "__setattr__",
                    "__delattr__",
                }
                for method in methods_in_body
            ):
                dynamic = True

            if methods_in_body:

                class FieldCollector(ast.NodeVisitor):
                    def __init__(
                        self,
                        add: Callable[[str], None],
                        add_hint: Callable[[str, ast.AST | None], None],
                        self_name: str = "self",
                    ) -> None:
                        self._add = add
                        self._add_hint = add_hint
                        self._self_name = self_name

                    def visit_Assign(self, node: ast.Assign) -> None:
                        for target in node.targets:
                            self._handle_target(target)
                        self.generic_visit(node.value)

                    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                        self._handle_target(node.target, node.annotation)
                        if node.value is not None:
                            self.generic_visit(node.value)

                    def _handle_target(
                        self, target: ast.AST, annotation: ast.AST | None = None
                    ) -> None:
                        if (
                            isinstance(target, ast.Attribute)
                            and isinstance(target.value, ast.Name)
                            and target.value.id == self._self_name
                        ):
                            self._add(target.attr)
                            self._add_hint(target.attr, annotation)

                    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                        return

                    def visit_AsyncFunctionDef(
                        self, node: ast.AsyncFunctionDef
                    ) -> None:
                        return

                    def visit_Lambda(self, node: ast.Lambda) -> None:
                        return

                for method in methods_in_body:
                    # Field discovery is for INSTANCE attributes — only
                    # regular instance methods feed the field layout.
                    # `@classmethod`, `@staticmethod`, and implicit-classmethod
                    # methods (`__new__`, `__init_subclass__`,
                    # `__class_getitem__`) take a class as their first
                    # argument; assignments through it set class attributes
                    # via the dict, never instance fields.
                    if not _function_is_instance_method(
                        method, self.python_binding_index
                    ):
                        continue
                    # Use the actual first parameter name (e.g. "self", "s")
                    # so that ``def __init__(s, x): s.x = x`` correctly
                    # discovers field ``x``.
                    self_param = "self"
                    if method.args.args:
                        self_param = method.args.args[0].arg
                    collector = FieldCollector(
                        add_field, add_field_hint, self_name=self_param
                    )
                    for stmt in method.body:
                        collector.visit(stmt)

            min_layout = self._builtin_min_layout(mro)
            reserved_tail = self._class_reserved_tail_size(mro)
            base_size = (
                (len(field_order) * 8 + reserved_tail) if not dynamic else reserved_tail
            )
            size = max(base_size, min_layout)
            self.classes[class_id] = ClassInfo(
                binding_name=node.name,
                fields=fields,
                size=size,
                methods=methods,
                pending_methods=pending_methods,
                field_order=field_order,
                defaults=field_defaults,
                field_hints=field_hints,
                class_attrs=class_attrs,
                possible_data_descriptor_attrs=self._possible_data_descriptor_attrs(
                    node, class_attrs
                ),
                bases=tuple(bases),
                mro=mro,
                dynamic=dynamic,
                static=is_static,
                needs_classcell=needs_classcell,
                custom_metaclass=has_metaclass_kw
                or inherits_custom_meta
                or dynamic_build,
                decorated=bool(other_decorators),
                constructor_binding_proven=constructor_binding_proven,
            )

        # Layout expectations come from immutable potential slots, not values
        # eagerly instantiated by a separate method-emission lane. Dynamic class
        # bodies use runtime layout and never turn conditional methods into
        # unconditional devirtualization targets.
        self.classes[class_id]["layout_version"] = self._class_layout_version(
            class_id,
            class_attrs,
            method_count=len(pending_methods),
        )
        classcell_val: MoltValue | None = None
        if needs_classcell:
            empty = self._emit_missing_value()
            classcell_val = self._emit_cell_new(empty)

        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(
            MoltOp(
                kind="CONST_STR", args=[python_definition_name(node)], result=name_val
            )
        )
        qualname = self._definition_qualname(node)
        if qualname != class_id.qualname:
            raise FrontendRejection(
                Diagnostic.INTERNAL_INVARIANT,
                f"class {qualname!r} lowered under ClassId qualname "
                f"{class_id.qualname!r}",
            )
        qualname_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[qualname], result=qualname_val))
        module_name = "__main__" if self.is_entry_module else self.module_name
        module_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=module_val))

        dynamic_namespace: MoltValue | None = None
        # ``classcell_val`` is created earlier (before the method loop) when
        # ``needs_classcell`` so it can be threaded into method closures; do not
        # re-declare it here or the pre-created cell would be lost.
        dynamic_bases_tuple: MoltValue | None = None
        dynamic_meta: MoltValue | None = None
        dynamic_prepared_kwds: MoltValue | None = None
        dynamic_kwds: MoltValue | None = None
        if dynamic_build:
            if node.keywords:
                # __build_class__ has the same keyword assembly contract as a
                # normal Python call, before base resolution/metaclass entry.
                keyword_call = ast.Call(
                    func=ast.Name(id="dict", ctx=ast.Load()),
                    args=[],
                    keywords=node.keywords,
                )
                dictionary = self._emit_builtin_type_value("dict")
                keyword_args = self._emit_call_args_builder(keyword_call)
                dynamic_kwds = MoltValue(self.next_var(), type_hint="dict")
                self.emit(
                    MoltOp(
                        kind="CALL_BIND",
                        args=[dictionary, keyword_args],
                        result=dynamic_kwds,
                    )
                )

            if expanded_bases is not None:
                dynamic_bases_tuple = expanded_bases
            elif has_explicit_bases:
                bases_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=base_vals, result=bases_tuple))
                dynamic_bases_tuple = bases_tuple
            else:
                empty_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=empty_tuple))
                dynamic_bases_tuple = empty_tuple

            if dynamic_bases_tuple is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported class bases"
                )
            types_bootstrap = self._emit_intrinsic_call(
                "molt_types_bootstrap", [], type_hint="dict"
            )
            resolve_key = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="CONST_STR", args=["resolve_bases"], result=resolve_key)
            )
            resolve_bases_func = MoltValue(self.next_var(), type_hint="function")
            self.emit(
                MoltOp(
                    kind="INDEX",
                    args=[types_bootstrap, resolve_key],
                    result=resolve_bases_func,
                )
            )
            resolve_args = MoltValue(self.next_var(), type_hint="callargs")
            self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=resolve_args))
            self.emit(
                MoltOp(
                    kind="CALLARGS_PUSH_POS",
                    args=[resolve_args, dynamic_bases_tuple],
                    result=MoltValue("none"),
                )
            )
            dynamic_bases_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(
                    kind="CALL_BIND",
                    args=[resolve_bases_func, resolve_args],
                    result=dynamic_bases_tuple,
                )
            )

            none_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
            kwds_val = dynamic_kwds if dynamic_kwds is not None else none_val

            prepare_key = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="CONST_STR", args=["prepare_class"], result=prepare_key)
            )
            prepare_class_func = MoltValue(self.next_var(), type_hint="function")
            self.emit(
                MoltOp(
                    kind="INDEX",
                    args=[types_bootstrap, prepare_key],
                    result=prepare_class_func,
                )
            )
            prepare_args = MoltValue(self.next_var(), type_hint="callargs")
            self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=prepare_args))
            self.emit(
                MoltOp(
                    kind="CALLARGS_PUSH_POS",
                    args=[prepare_args, name_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(
                    kind="CALLARGS_PUSH_POS",
                    args=[prepare_args, dynamic_bases_tuple],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(
                    kind="CALLARGS_PUSH_POS",
                    args=[prepare_args, kwds_val],
                    result=MoltValue("none"),
                )
            )
            prepared_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(
                    kind="CALL_BIND",
                    args=[prepare_class_func, prepare_args],
                    result=prepared_tuple,
                )
            )

            zero_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero_val))
            one_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one_val))
            two_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[2], result=two_val))

            dynamic_meta = MoltValue(self.next_var(), type_hint="type")
            self.emit(
                MoltOp(
                    kind="INDEX", args=[prepared_tuple, zero_val], result=dynamic_meta
                )
            )
            namespace_val = MoltValue(self.next_var(), type_hint="dict")
            self.emit(
                MoltOp(
                    kind="INDEX", args=[prepared_tuple, one_val], result=namespace_val
                )
            )
            dynamic_prepared_kwds = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="INDEX",
                    args=[prepared_tuple, two_val],
                    result=dynamic_prepared_kwds,
                )
            )

            dynamic_namespace = namespace_val

        saved_locals = self.locals
        self.locals = {}
        self._class_body_depth += 1
        # Block-execution scope for the class body (P0 #50).  ``_store_local_value``
        # / ``_load_local_value`` / ``_emit_delete_name`` consult the top of
        # ``self._class_ns_stack`` so a class-scope name binds into / reads from
        # the namespace mapping (when ``dynamic_namespace`` exists) instead of the
        # enclosing function frame.  ``attr_values`` is shared with
        # ``class_attr_values`` (the build path's view); ``names`` is seeded with
        # the names already bound (methods + any class attrs harvested above) so
        # in-body loads of those resolve to the class namespace, while an unbound
        # Name still falls through to global/builtin resolution (CPython
        # LOAD_NAME).  The straight-line arms below ALSO bind through
        # ``_class_ns_store`` (via ``bind_class_name``) so there is exactly one
        # code path that publishes a class-body name.
        class_ns_scope = _ClassNsScope(
            ns=dynamic_namespace,
            attr_values=class_attr_values,
            names=(
                set(class_attr_values)
                | set(methods)
                | self._collect_assigned_names(node.body)
            ),
            class_name=node.name,
            module_name=module_name,
            class_node=node,
            class_cell=classcell_val,
            methods=methods,
            local_names=frozenset(self._collect_assigned_names(node.body)),
            global_names=frozenset(self._collect_global_decls(node.body)),
            nonlocal_names=frozenset(self._collect_nonlocal_decls(node.body)),
            enclosing_locals=(
                self._class_ns_stack[-1].enclosing_locals
                if self._class_ns_stack
                else saved_locals
            ),
        )

        def bind_class_name(name: str, value: MoltValue) -> None:
            # Single source of truth for "this straight-line arm bound a
            # class-body attribute": update the SSA fast-path view, mirror into
            # the namespace dict (when present), and keep the enclosing-frame
            # ``self.locals`` cache coherent for the rare in-body load that
            # predates control flow.
            self._class_ns_store(class_ns_scope, name, value)
            self.locals[name] = value

        # All consumers share one namespace owner. Static bodies retain an SSA
        # projection until a deferred evaluator requires a captured mapping.
        class_import_state = self._capture_class_import_state()
        self._class_ns_stack.append(class_ns_scope)
        self._push_qualname(python_definition_name(node), False, qualname=qualname)
        python_frame_scope = self._enter_python_frame_context_scope(class_body=True)
        class_body_complete = False
        try:
            if dynamic_namespace is not None:
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=["__module__"], result=key_val))
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[dynamic_namespace, key_val, module_val],
                        result=MoltValue("none"),
                    )
                )
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(kind="CONST_STR", args=["__qualname__"], result=key_val)
                )
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[dynamic_namespace, key_val, qualname_val],
                        result=MoltValue("none"),
                    )
                )
            if dynamic_namespace is not None and source_keys:
                # __firstlineno__: the line number of the class statement.
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(kind="CONST_STR", args=["__firstlineno__"], result=key_val)
                )
                lineno_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[node.lineno], result=lineno_val))
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[dynamic_namespace, key_val, lineno_val],
                        result=MoltValue("none"),
                    )
                )
            self._setup_class_annotations(node, class_ns_scope)
            for item in node.body:
                if isinstance(item, (ast.Global, ast.Nonlocal)):
                    # Class directives belong to this namespace, never to the
                    # enclosing function frame used to lower its body.
                    continue
                if isinstance(item, ast.TypeAlias):
                    self.visit_TypeAlias(item)
                    continue
                if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    self.visit(item)
                    continue
                if isinstance(item, ast.Expr):
                    if isinstance(item.value, ast.Constant) and isinstance(
                        item.value.value, str
                    ):
                        continue
                    if self.visit(item.value) is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE,
                            "Unsupported class body expression",
                        )
                    continue
                if isinstance(item, ast.Assign) and all(
                    isinstance(t, ast.Name) for t in item.targets
                ):
                    val = self.visit(item.value)
                    if val is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE,
                            "Unsupported class body assignment",
                        )
                    for target in item.targets:
                        assert isinstance(target, ast.Name)
                        bind_class_name(target.id, val)
                    continue
                if isinstance(item, ast.Pass):
                    continue
                # Any remaining class-body statement — control flow
                # (for/if/while/try/with/match), ``del``, augmented assignment,
                # tuple-unpack assignment, import, etc. — is lowered as an
                # ORDINARY statement over the class namespace (P0 #50).  Because
                # ``self._class_ns_stack`` is active, every name STORE/LOAD/DELETE
                # inside ``self.visit(item)`` funnels through ``_store_local_value``
                # / ``_load_local_value`` / ``_emit_delete_name`` and routes to the
                # class namespace mapping, so the statement "just works" exactly as
                # CPython executes the class-body code object.  ``body_needs_block``
                # guaranteed a real ``dynamic_namespace`` exists for these.
                self.visit(item)
            if (
                not self.future_annotations
                and not self.eager_annotations
                and self.class_annotation_items
            ):
                annotate_value = self._emit_annotate_function_obj(
                    items=self.class_annotation_items,
                    exec_map_name=None,
                    exec_map=self.class_annotation_exec_map,
                    stringize=False,
                    module_override=module_name,
                    class_scope=class_ns_scope,
                )
                self._class_ns_store(
                    class_ns_scope, "__annotate_func__", annotate_value
                )
            # __static_attributes__ is emitted after the class body, even when
            # empty. It follows the methods in namespace event order.
            if dynamic_namespace is not None and source_keys:
                static_attrs = self._collect_static_attributes(node)
                attr_vals: list[MoltValue] = []
                for attr_name in static_attrs:
                    av = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[attr_name], result=av))
                    attr_vals.append(av)
                static_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=attr_vals, result=static_tuple))
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(
                        kind="CONST_STR", args=["__static_attributes__"], result=key_val
                    )
                )
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[dynamic_namespace, key_val, static_tuple],
                        result=MoltValue("none"),
                    )
                )

            if classcell_val is not None:
                self._class_ns_store(class_ns_scope, "__classcell__", classcell_val)
            if class_ns_scope.annotation_namespace_cell is not None:
                self._class_ns_store(
                    class_ns_scope,
                    "__classdictcell__",
                    class_ns_scope.annotation_namespace_cell,
                )
            class_body_complete = True
        finally:
            self._pop_qualname()
            self._class_body_depth -= 1
            self.locals = saved_locals
            popped_scope = self._class_ns_stack.pop()
            assert popped_scope is class_ns_scope, "class-ns scope stack imbalance"
            self._restore_class_import_state(
                class_import_state,
                class_ns_scope.global_names,
                class_ns_scope.nonlocal_names,
            )
            self._exit_python_frame_context_scope(
                python_frame_scope, emit_cleanup=class_body_complete
            )

        if dynamic_build:
            if (
                dynamic_meta is None
                or dynamic_bases_tuple is None
                or dynamic_namespace is None
            ):
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported dynamic class build"
                )
            callargs = MoltValue(self.next_var(), type_hint="callargs")
            self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
            self.emit(
                MoltOp(
                    kind="CALLARGS_PUSH_POS",
                    args=[callargs, name_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(
                    kind="CALLARGS_PUSH_POS",
                    args=[callargs, dynamic_bases_tuple],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(
                    kind="CALLARGS_PUSH_POS",
                    args=[callargs, dynamic_namespace],
                    result=MoltValue("none"),
                )
            )
            if dynamic_prepared_kwds is not None:
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                kwds_is_none = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(
                        kind="IS",
                        args=[dynamic_prepared_kwds, none_val],
                        result=kwds_is_none,
                    )
                )
                self.emit(
                    MoltOp(kind="IF", args=[kwds_is_none], result=MoltValue("none"))
                )
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_KWSTAR",
                        args=[callargs, dynamic_prepared_kwds],
                        result=MoltValue(self.next_var(), type_hint="None"),
                    )
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            # A user metaclass may return an arbitrary object.
            class_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="CALL_BIND",
                    args=[dynamic_meta, callargs],
                    result=class_val,
                )
            )
            if classcell_val is not None:
                self._verify_classcell_result(class_id.name, classcell_val, class_val)
        else:
            # Outlined class definition: collect attrs, emit single CLASS_DEF op
            class_def_attrs: list[tuple[MoltValue, MoltValue]] = []
            source_attrs = [("__qualname__", qualname_val), ("__module__", module_val)]
            if source_keys:
                lineno_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[node.lineno], result=lineno_val))
                source_attrs.append(("__firstlineno__", lineno_val))
            for attr_str, attr_val in source_attrs:
                key = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[attr_str], result=key))
                class_def_attrs.append((key, attr_val))
            static_attrs = self._collect_static_attributes(node) if source_keys else ()
            if static_attrs:
                sa_vals: list[MoltValue] = []
                for sa_name in static_attrs:
                    sv = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[sa_name], result=sv))
                    sa_vals.append(sv)
                sa_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=sa_vals, result=sa_tuple))
                sa_key = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(
                        kind="CONST_STR", args=["__static_attributes__"], result=sa_key
                    )
                )
                class_def_attrs.append((sa_key, sa_tuple))
            class_val = MoltValue(self.next_var(), type_hint="type")

        class_info = self.classes[class_id]
        if not dynamic_build:
            # Collect field offsets into attrs
            if (
                not class_info.get("dataclass")
                and not class_info.get("dynamic")
                and class_info.get("fields")
            ):
                field_items: list[MoltValue] = []
                for field in sorted(class_info["fields"]):
                    key_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[field], result=key_val))
                    offset_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(
                            kind="CONST",
                            args=[class_info["fields"][field]],
                            result=offset_val,
                        )
                    )
                    field_items.extend([key_val, offset_val])
                offsets_dict = MoltValue(self.next_var(), type_hint="dict")
                self.emit(
                    MoltOp(kind="DICT_NEW", args=field_items, result=offsets_dict)
                )
                fkey = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(
                        kind="CONST_STR", args=["__molt_field_offsets__"], result=fkey
                    )
                )
                class_def_attrs.append((fkey, offsets_dict))
            for attr_name, val in class_attr_values.items():
                akey = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[attr_name], result=akey))
                class_def_attrs.append((akey, val))
            class_def_args: list[Any] = [name_val] + list(base_vals)
            for k, v in class_def_attrs:
                class_def_args.append(k)
                class_def_args.append(v)
            layout_version = class_info.get("layout_version", 0)
            class_def_flags = 1 if base_vals else 0
            class_def_meta = f"{len(base_vals)},{len(class_def_attrs)},{class_info['size']},{layout_version},{class_def_flags}"
            self.emit(
                MoltOp(
                    kind="CLASS_DEF",
                    args=class_def_args,
                    result=class_val,
                    metadata={"s_value": class_def_meta},
                )
            )
            # The statement's identity, stamped on the class it just created
            # before any binding, decorator or dataclass transform sees it
            # (HF-220). Layout guards compare it; the metaclass path below
            # builds only dynamic classes, which no guard trusts.
            definition = layout_definition_symbol(class_id, class_info)
            if definition is not None:
                self._register_code_symbol(definition)
                self.emit(
                    MoltOp(
                        kind="CLASS_SET_DEFINITION",
                        args=[class_val],
                        result=MoltValue("none"),
                        metadata={"class_definition": definition},
                    )
                )
            if classcell_val is not None:
                self._verify_classcell_result(class_id.name, classcell_val, class_val)
        # Static and dynamic construction return a value. Dataclass rebuilding
        # and ordinary decorators transform it before the defining scope sees
        # one final binding, including custom class namespace mappings.
        if type_param_vals:
            self._emit_attach_type_params(class_val, type_param_vals)
            class_getitem = self._emit_module_attr_get_on(
                "typing", "_molt_class_getitem"
            )
            wrapped = MoltValue(self.next_var(), type_hint="classmethod")
            self.emit(
                MoltOp(kind="CLASSMETHOD_NEW", args=[class_getitem], result=wrapped)
            )
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[class_val, "__class_getitem__", wrapped],
                    result=MoltValue("none"),
                )
            )

        if decorator_vals:
            decorated = class_val
            for decorator_val in reversed(decorator_vals):
                callargs = MoltValue(self.next_var(), type_hint="callargs")
                self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
                push_res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_PUSH_POS",
                        args=[callargs, decorated],
                        result=push_res,
                    )
                )
                res = MoltValue(
                    self.next_var(),
                    type_hint="type"
                    if dataclass_opts is not None and not other_decorators
                    else "Any",
                )
                self.emit(
                    MoltOp(kind="CALL_BIND", args=[decorator_val, callargs], result=res)
                )
                decorated = res
            class_val = decorated

        self._publish_definition_binding(node.name, class_val)

        bound_class = self.globals.get(node.name)
        if (
            self.current_func_name == "molt_main"
            and not other_decorators
            and not dynamic_build
            and bound_class is not None
            and bound_class.name == class_val.name
            and (
                not class_info.get("dataclass")
                or not class_info.get("dataclass_params", {}).get("slots", False)
            )
        ):
            class_info["class_value_name"] = class_val.name
            if self._class_constructor_fold_safe(class_id, class_info):
                class_info["constructor_fold_safe"] = True
            else:
                class_info.pop("constructor_fold_safe", None)
        else:
            class_info.pop("class_value_name", None)
            class_info.pop("constructor_fold_safe", None)

        return None
