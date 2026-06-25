"""ClassDefVisitorMixin: class-definition lowering (F1 decomposition).

Move-only extraction from frontend/__init__.py (F1 phase 2). Covers
visit_ClassDef and its exclusively-owned class/MRO/descriptor/inline-init and
method-closure helpers (every method here is, transitively, called only from
within this family). self.<method> / self.<attr> references resolve through the
SimpleTIRGenerator MRO at runtime.
"""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    Any,
    Callable,
    Literal,
    cast,
)

from molt.frontend._types import (
    BUILTIN_EXCEPTION_NAMES,
    BUILTIN_LAYOUT_MIN,
    BUILTIN_TYPE_TAGS,
    ClassInfo,
    GEN_CLOSED_OFFSET,
    GEN_CONTROL_SIZE,
    MethodInfo,
    MoltOp,
    MoltValue,
    _ClassNsScope,
    _MOLT_CLOSURE_PARAM,
    _function_is_instance_method,
    _next_ic_index,
)
from molt.frontend.sema import (
    async_generator_contains_return_value,
    async_generator_contains_yield_from,
    function_contains_yield,
    signature_contains_yield,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


def _iter_slots_field_names(value: ast.expr | None) -> list[str]:
    """Field names declared by a ``__slots__`` assignment that consume an instance
    field slot.

    Accepts the literal forms ``__slots__`` is normally given — a single string,
    or a tuple/list/set of string literals. ``__dict__`` and ``__weakref__`` are
    excluded because the runtime's ``apply_class_slots_layout`` does not assign
    them a field offset (they toggle instance-dict / weakref support instead), so
    the frontend's slot-size accounting must skip them in lock-step to keep
    ``class_info["size"]`` equal to the runtime's ``class_layout_size``.
    Non-literal ``__slots__`` (a computed expression) yields no names; such a
    class falls back to the runtime layout authority unchanged.
    """
    if value is None:
        return []
    if isinstance(value, ast.Constant) and isinstance(value.value, str):
        elements: list[ast.expr] = [value]
    elif isinstance(value, (ast.Tuple, ast.List, ast.Set)):
        elements = list(value.elts)
    else:
        return []
    names: list[str] = []
    for element in elements:
        if isinstance(element, ast.Constant) and isinstance(element.value, str):
            name = element.value
            if name in ("__dict__", "__weakref__"):
                continue
            names.append(name)
    return names


class ClassDefVisitorMixin(_MixinBase):
    # Statement node types a class body can hold and have lowered by the
    # dedicated straight-line arms of ``visit_ClassDef`` (attribute bindings and
    # method/nested-class definitions).  ANY other top-level body statement —
    # control flow (For/AsyncFor/If/While/With/AsyncWith/Try/TryStar/Match),
    # ``del`` (Delete), augmented assignment (AugAssign), import, etc. — requires
    # executing the body as a normal block over the class namespace mapping
    # (CPython semantics).  ``Pass`` is inert.  (P0 #50.)
    _CLASS_BODY_SIMPLE_STMTS = (
        ast.FunctionDef,
        ast.AsyncFunctionDef,
        ast.ClassDef,
        ast.Assign,
        ast.AnnAssign,
        ast.Expr,
        ast.Pass,
    )

    @classmethod
    def _class_body_needs_block_exec(cls, body: list[ast.stmt]) -> bool:
        """True when a class body holds control flow / ``del`` (P0 #50).

        Only the *top-level* class-body statements matter: control flow nested
        inside a method body or a comprehension is the method's/comprehension's
        own scope, not the class block.  An ``AnnAssign`` / ``Assign`` whose
        target is not a bare ``Name`` (e.g. ``obj.attr = x`` or ``a, b = t`` —
        a tuple-unpack the straight-line arms do not handle) also forces block
        execution so it routes through the full assignment lowering.
        """
        for stmt in body:
            if not isinstance(stmt, cls._CLASS_BODY_SIMPLE_STMTS):
                return True
            if isinstance(stmt, ast.Assign):
                if any(not isinstance(t, ast.Name) for t in stmt.targets):
                    return True
            elif isinstance(stmt, ast.AnnAssign):
                if not isinstance(stmt.target, ast.Name):
                    return True
        return False

    def _emit_dataclass_application(
        self,
        node: ast.ClassDef,
        class_info: ClassInfo,
        class_val: MoltValue,
    ) -> MoltValue:
        """Emit the compile-time-recognized ``@dataclass`` runtime application.

        The ``@dataclass`` transform is construction-method-agnostic: it operates
        on a *finished* class object via ``setattr`` / ``cls.x = ...`` and reads
        ``cls.__annotations__`` (gathered at compile time and published into the
        class namespace).  It therefore applies identically whether ``class_val``
        came from the static "outlined ``CLASS_DEF``" path or from the dynamic
        metaclass-call path the #50 block-execution re-lower uses.  Centralizing
        the emission here keeps exactly ONE code path that publishes the dataclass
        transform, so a dataclass whose body needs block execution (control flow /
        ``del`` / non-Name assign target) still gets its generated dunders.

        When ``class_info`` is not a dataclass this is a no-op and returns
        ``class_val`` unchanged.  Otherwise it emits the
        ``dataclasses.dataclass(cls, init=..., repr=..., ...)`` call, rebinds the
        class name to the (possibly rebuilt — e.g. ``slots=True``) result, and
        returns that new value.
        """
        if not class_info.get("dataclass"):
            return class_val

        dataclass_params = class_info.get("dataclass_params", {})

        def emit_bool(value: bool) -> MoltValue:
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[value], result=res))
            return res

        # Route the compile-time-recognized ``@dataclass`` path through the
        # public ``dataclasses.dataclass`` wrapper rather than the internal
        # ``_molt_apply_dataclass`` worker.  The wrapper performs the same work
        # but its calling convention — single positional ``cls`` plus keyword-only
        # options — matches the natural Python semantics, avoiding an 11-argument
        # positional-only call into the worker that exposed an SSA/dominator
        # interaction during module init (frontend bypass would intermittently
        # corrupt the class binding before module attribute publication).
        kw_specs = [
            ("init", emit_bool(dataclass_params.get("init", True))),
            ("repr", emit_bool(dataclass_params.get("repr", True))),
            ("eq", emit_bool(dataclass_params.get("eq", True))),
            ("order", emit_bool(dataclass_params.get("order", False))),
            ("unsafe_hash", emit_bool(dataclass_params.get("unsafe_hash", False))),
            ("frozen", emit_bool(dataclass_params.get("frozen", False))),
            ("match_args", emit_bool(dataclass_params.get("match_args", True))),
            ("kw_only", emit_bool(dataclass_params.get("kw_only", False))),
            ("slots", emit_bool(dataclass_params.get("slots", False))),
            ("weakref_slot", emit_bool(dataclass_params.get("weakref_slot", False))),
        ]
        helper_val = self._emit_module_attr_get_on("dataclasses", "dataclass")
        callargs = MoltValue(self.next_var(), type_hint="callargs")
        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
        # Single positional argument: the class itself.
        self.emit(
            MoltOp(
                kind="CALLARGS_PUSH_POS",
                args=[callargs, class_val],
                result=MoltValue("none"),
            )
        )
        # Keyword-only options matching CPython's dataclass signature.
        for kw_name, kw_val in kw_specs:
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[kw_name], result=key_val))
            self.emit(
                MoltOp(
                    kind="CALLARGS_PUSH_KW",
                    args=[callargs, key_val, kw_val],
                    result=MoltValue("none"),
                )
            )
        # ``dataclass`` always returns the (possibly rebuilt) class object.
        # Capture and rebind so that ``slots=True`` — which produces a brand-new
        # class via ``_add_slots`` — and any future rebuild paths replace the
        # original binding.  For the non-slots path the function mutates and
        # returns the same object, so the rebind is a no-op.
        applied_cls = MoltValue(self.next_var(), type_hint="type")
        self.emit(
            MoltOp(
                kind="CALL_BIND",
                args=[helper_val, callargs],
                result=applied_cls,
            )
        )
        self._publish_class_value(node.name, applied_cls)
        return applied_cls

    def _property_field_from_method(self, node: ast.FunctionDef) -> str | None:
        if len(node.body) != 1:
            return None
        stmt = node.body[0]
        if not isinstance(stmt, ast.Return):
            return None
        value = stmt.value
        if not isinstance(value, ast.Attribute):
            return None
        if not isinstance(value.value, ast.Name):
            return None
        if value.value.id != "self":
            return None
        return value.attr

    def _class_layout_version(
        self,
        class_name: str,
        class_attrs: dict[str, ast.expr],
        methods: dict[str, MethodInfo] | None = None,
        method_count: int | None = None,
    ) -> int:
        class_info = self.classes[class_name]
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
        self, class_name: str, class_info: ClassInfo
    ) -> bool:
        if class_info.get("module") != self.module_name:
            return False
        if class_name not in self.stable_module_classes:
            return False
        if self.module_globals_dict_escaped:
            return False
        if class_name in self.module_global_mutations:
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
        if self._class_defines_finalizer(class_name):
            return False
        # `object_new_bound` and the inlined-init constructor fold are only
        # equivalent to `type.__call__` when the MRO resolves `__new__` to
        # default `object.__new__`.  Custom or opaque `__new__` must stay on the
        # runtime class-call route so inherited overrides consume constructor
        # args and decide whether `__init__` should run.
        if not self._class_resolves_default_object_new(class_name, class_info):
            return False
        return self._class_layout_stable(class_name)

    def _class_defines_finalizer(self, class_name: str) -> bool:
        """True iff ``class_name`` resolves a user-defined ``__del__`` through its
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
        for name in self._class_mro_names(class_name):
            if name == "object":
                continue
            info = self.classes.get(name)
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

    def _builtin_min_layout(self, mro_names: list[str]) -> int:
        min_size = 0
        for name in mro_names:
            min_size = max(min_size, BUILTIN_LAYOUT_MIN.get(name, 0))
        return min_size

    def _class_reserved_tail_size(self, mro_names: list[str]) -> int:
        return 16 if "dict" in mro_names else 8

    def _ensure_class_annotation_exec_map(self, class_name: str) -> MoltValue:
        if self.class_annotation_exec_map is not None:
            return self.class_annotation_exec_map
        owner = self._sanitize_module_name(class_name)
        name = self._annotation_exec_name(owner)
        self.class_annotation_exec_name = name
        exec_map = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=exec_map))
        self.class_annotation_exec_map = exec_map
        self._store_local_value(name, exec_map)
        if self.current_func_name.startswith("molt_init_"):
            self.globals[name] = exec_map
            self._emit_module_attr_set(name, exec_map)
        return exec_map

    def _rewrite_class_annotation_expr(
        self, expr: ast.expr, class_name: str, class_scope: set[str]
    ) -> ast.expr:
        class_name_node = ast.Name(id=class_name, ctx=ast.Load())

        class Rewriter(ast.NodeTransformer):
            def visit_Name(self, node: ast.Name) -> ast.AST:
                if isinstance(node.ctx, ast.Load) and node.id in class_scope:
                    return ast.copy_location(
                        ast.Attribute(
                            value=class_name_node,
                            attr=node.id,
                            ctx=ast.Load(),
                        ),
                        node,
                    )
                return node

            def visit_Lambda(self, node: ast.Lambda) -> ast.AST:
                return node

            def visit_FunctionDef(self, node: ast.FunctionDef) -> ast.AST:
                return node

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> ast.AST:
                return node

            def visit_ClassDef(self, node: ast.ClassDef) -> ast.AST:
                return node

        return cast(ast.expr, Rewriter().visit(expr))

    def _emit_function_defaults(
        self,
        func_val: MoltValue,
        default_exprs: list[ast.expr],
        kw_default_exprs: list[ast.expr | None],
        kwonly_params: list[str],
    ) -> MoltValue:
        func_val, defaults_val, kwdefaults_val = self._emit_function_default_values(
            func_val, default_exprs, kw_default_exprs, kwonly_params
        )
        setter = self._emit_runtime_function("molt_function_set_defaults", 3)
        res = MoltValue(self.next_var(), type_hint="None")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[setter, func_val, defaults_val, kwdefaults_val],
                result=res,
            )
        )
        return func_val

    def _collect_static_attributes(self, class_node: ast.ClassDef) -> tuple[str, ...]:
        """Collect attribute names set via self.X = ... in class body methods.

        Returns a tuple of unique attribute names in definition order,
        matching CPython 3.13+ __static_attributes__.
        """
        attrs: list[str] = []
        seen: set[str] = set()

        class SelfAttrCollector(ast.NodeVisitor):
            def __init__(self, self_name: str) -> None:
                self.self_name = self_name

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    self._check(target)
                self.generic_visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                self._check(node.target)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                self._check(node.target)

            def _check(self, target: ast.AST) -> None:
                if (
                    isinstance(target, ast.Attribute)
                    and isinstance(target.value, ast.Name)
                    and target.value.id == self.self_name
                    and target.attr not in seen
                ):
                    seen.add(target.attr)
                    attrs.append(target.attr)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return  # Don't recurse into nested functions

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return  # Don't recurse into nested classes

        for item in class_node.body:
            if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                # CPython's `__static_attributes__` only records attributes
                # assigned via `self.X = ...` from regular instance methods.
                # `@classmethod`, `@staticmethod`, and the implicit-classmethod
                # methods (`__new__`, `__init_subclass__`, `__class_getitem__`)
                # do NOT contribute — their first parameter binds to the class
                # itself, not to an instance.
                if not _function_is_instance_method(item):
                    continue
                # First parameter is self
                self_name = "self"
                if item.args.args:
                    self_name = item.args.args[0].arg
                collector = SelfAttrCollector(self_name)
                for stmt in item.body:
                    collector.visit(stmt)
            elif isinstance(item, ast.AnnAssign) and isinstance(item.target, ast.Name):
                # Class-level annotations like x: int
                name = item.target.id
                if name not in seen:
                    seen.add(name)
                    attrs.append(name)

        return tuple(attrs)

    def _publish_class_value(self, name: str, class_val: MoltValue) -> None:
        """Bind a freshly built class object into its defining scope.

        Single source of truth for the four ``visit_ClassDef`` publication
        paths (static / dataclass-rebuilt / dynamic / decorated).  A nested
        ``class`` statement (``self._class_body_depth > 0``) is a member of the
        enclosing class body, exactly like a method or a class-attribute
        assignment.  Those bind into the class-body namespace with a *direct*
        ``self.locals[name] = value`` write (see the method and ``ast.Assign``
        branches in the body loop) rather than the function-local store
        machinery (boxed cells / async closure slots), and they are never
        published to module globals — even when the outermost enclosing class
        lives at module scope.  The enclosing class-body loop harvests this
        binding into ``class_attr_values``; here we only need a deterministic
        ``self.locals`` entry, so we mirror that direct write.
        """

        if self._class_body_depth > 0:
            self.locals[name] = class_val
            return
        if self.current_func_name == "molt_main":
            self.globals[name] = class_val
            self._emit_module_attr_set(name, class_val)
            if name in self.boxed_locals:
                self._store_local_value(name, class_val)
        else:
            self._store_local_value(name, class_val)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        self.local_class_names.add(node.name)
        prev_class_annotations = self.class_annotation_items
        prev_class_exec_map = self.class_annotation_exec_map
        prev_class_exec_name = self.class_annotation_exec_name
        prev_class_exec_counter = self.class_annotation_exec_counter
        self.class_annotation_items = []
        self.class_annotation_exec_map = None
        self.class_annotation_exec_name = None
        self.class_annotation_exec_counter = 0
        dataclass_opts = None
        other_decorators: list[ast.expr] = []
        if node.decorator_list:
            for deco in node.decorator_list:
                if isinstance(deco, ast.Name) and deco.id == "dataclass":
                    if dataclass_opts is not None:
                        raise NotImplementedError(
                            "Multiple dataclass decorators are not supported"
                        )
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
                    continue
                if (
                    isinstance(deco, ast.Attribute)
                    and isinstance(deco.value, ast.Name)
                    and deco.value.id == "dataclasses"
                    and deco.attr == "dataclass"
                ):
                    if dataclass_opts is not None:
                        raise NotImplementedError(
                            "Multiple dataclass decorators are not supported"
                        )
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
                    continue
                if (
                    isinstance(deco, ast.Call)
                    and isinstance(deco.func, ast.Name)
                    and deco.func.id == "dataclass"
                ):
                    if dataclass_opts is not None:
                        raise NotImplementedError(
                            "Multiple dataclass decorators are not supported"
                        )
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
                    _DATACLASS_VALID_OPTS = {
                        "init",
                        "repr",
                        "eq",
                        "order",
                        "unsafe_hash",
                        "frozen",
                        "match_args",
                        "kw_only",
                        "slots",
                        "weakref_slot",
                    }
                    for kw in deco.keywords:
                        if kw.arg is None:
                            # **kwargs spread — resolve from module-level constant dicts
                            resolved = False
                            if isinstance(kw.value, ast.Name):
                                varname = kw.value.id
                                if varname in self.module_const_dicts:
                                    for dk, dv in self.module_const_dicts[
                                        varname
                                    ].items():
                                        if dk in _DATACLASS_VALID_OPTS and isinstance(
                                            dv, bool
                                        ):
                                            dataclass_opts[dk] = dv
                                    resolved = True
                            if resolved:
                                continue
                            raise NotImplementedError(
                                "dataclass **kwargs spread: cannot resolve '"
                                + (
                                    kw.value.id
                                    if isinstance(kw.value, ast.Name)
                                    else "?"
                                )
                                + "' at compile time. Define it as a module-level "
                                + "constant dict (e.g., OPTS = {'slots': True})"
                            )
                        if kw.arg not in _DATACLASS_VALID_OPTS:
                            # Unknown option — skip it (CPython would raise TypeError
                            # but we prefer to compile and let the runtime handle it)
                            continue
                        if (
                            isinstance(kw.value, ast.Constant)
                            and kw.value.value is None
                        ):
                            # None means "use the default" in CPython
                            continue
                        if not isinstance(kw.value, ast.Constant) or not isinstance(
                            kw.value.value, bool
                        ):
                            raise NotImplementedError(
                                f"dataclass {kw.arg} must be a boolean literal"
                            )
                        dataclass_opts[kw.arg] = kw.value.value
                    continue
                if (
                    isinstance(deco, ast.Call)
                    and isinstance(deco.func, ast.Attribute)
                    and isinstance(deco.func.value, ast.Name)
                    and deco.func.value.id == "dataclasses"
                    and deco.func.attr == "dataclass"
                ):
                    if dataclass_opts is not None:
                        raise NotImplementedError(
                            "Multiple dataclass decorators are not supported"
                        )
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
                    _DATACLASS_VALID_OPTS2 = {
                        "init",
                        "repr",
                        "eq",
                        "order",
                        "unsafe_hash",
                        "frozen",
                        "match_args",
                        "kw_only",
                        "slots",
                        "weakref_slot",
                    }
                    for kw in deco.keywords:
                        if kw.arg is None:
                            resolved = False
                            if isinstance(kw.value, ast.Name):
                                varname = kw.value.id
                                if varname in self.module_const_dicts:
                                    for dk, dv in self.module_const_dicts[
                                        varname
                                    ].items():
                                        if dk in _DATACLASS_VALID_OPTS2 and isinstance(
                                            dv, bool
                                        ):
                                            dataclass_opts[dk] = dv
                                    resolved = True
                            if resolved:
                                continue
                            raise NotImplementedError(
                                "dataclass **kwargs spread: cannot resolve '"
                                + (
                                    kw.value.id
                                    if isinstance(kw.value, ast.Name)
                                    else "?"
                                )
                                + "' at compile time. Define it as a module-level "
                                + "constant dict (e.g., OPTS = {'slots': True})"
                            )
                        if (
                            isinstance(kw.value, ast.Constant)
                            and kw.value.value is None
                        ):
                            # None means "use the default" in CPython
                            continue
                        if not isinstance(kw.value, ast.Constant) or not isinstance(
                            kw.value.value, bool
                        ):
                            raise NotImplementedError(
                                f"dataclass {kw.arg} must be a boolean literal"
                            )
                        dataclass_opts[kw.arg] = kw.value.value
                    continue
                other_decorators.append(deco)

        # @dataclass combined with other decorators is allowed.  Molt
        # processes @dataclass internally (innermost), then applies the
        # remaining decorators as outer wrappers — matching CPython
        # semantics for the common patterns (@final @dataclass, etc.).

        decorator_vals: list[MoltValue] = []
        if other_decorators:
            for deco in other_decorators:
                decorator_val = self.visit(deco)
                if decorator_val is None:
                    raise NotImplementedError("Unsupported class decorator")
                decorator_vals.append(decorator_val)

        type_param_vals, type_param_map = self._emit_type_params_values(
            getattr(node, "type_params", None)
        )
        prev_type_params = self.annotation_type_params
        if type_param_map:
            merged = dict(prev_type_params)
            merged.update(type_param_map)
            self.annotation_type_params = merged

        def base_expr_name(expr: ast.expr) -> str | None:
            if isinstance(expr, ast.Name):
                return expr.id
            if isinstance(expr, ast.Attribute):
                parts: list[str] = []
                current: ast.expr | None = expr
                while isinstance(current, ast.Attribute):
                    parts.append(current.attr)
                    current = current.value
                if isinstance(current, ast.Name):
                    parts.append(current.id)
                    parts.reverse()
                    return ".".join(parts)
            return None

        base_vals: list[MoltValue] = []
        base_names: list[str] = []
        base_name_lookup: list[str | None] = []
        has_explicit_bases = bool(node.bases)
        if node.bases:
            for base_expr in node.bases:
                prev_base_in_annotation = self.in_annotation
                if type_param_map:
                    self.in_annotation = True
                try:
                    base_val = self.visit(base_expr)
                finally:
                    self.in_annotation = prev_base_in_annotation
                if base_val is None:
                    raise NotImplementedError("Base class must be defined before use")
                base_vals.append(base_val)
                base_name = base_expr_name(base_expr)
                base_name_lookup.append(base_name)
                if base_name is not None:
                    base_names.append(base_name)

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
        for base_name in base_name_lookup:
            if base_name is None:
                dynamic_build = True
                continue
            base_info = self.classes.get(base_name)
            if (
                base_info is None
                and base_name not in BUILTIN_TYPE_TAGS
                and base_name not in BUILTIN_EXCEPTION_NAMES
            ):
                dynamic_build = True
                continue
            if base_info and base_info.get("custom_metaclass"):
                inherits_custom_meta = True
                dynamic_build = True
            if base_info and "__mro_entries__" in base_info.get("methods", {}):
                dynamic_build = True
        # A class body that contains anything beyond straight-line attribute
        # bindings / method-and-nested-class definitions (i.e. control flow —
        # for/if/while/try/with/match — or ``del``, or any augmented/looped
        # rebind of a class-scope name) must execute as a NORMAL BLOCK whose
        # mutable namespace is a real dict (CPython's class-body code object over
        # ``f_locals``).  Forcing ``dynamic_build`` gives that body a heap-backed
        # namespace mapping which is the loop-carried-correct store for its
        # names; the straight-line static fast path is untouched.  (P0 #50.)
        body_needs_block = self._class_body_needs_block_exec(node.body)
        if body_needs_block:
            dynamic_build = True
        if not has_explicit_bases:
            base_names = ["object"]

        if not base_vals and not dynamic_build:
            tag_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[BUILTIN_TYPE_TAGS["object"]], result=tag_val)
            )
            base_val = MoltValue(self.next_var(), type_hint="type")
            self.emit(MoltOp(kind="BUILTIN_TYPE", args=[tag_val], result=base_val))
            base_vals = [base_val]
            base_names = ["object"]

        methods: dict[str, MethodInfo] = {}
        needs_classcell = any(
            self._function_needs_classcell(item)
            for item in node.body
            if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef))
        )
        property_updates: dict[int, MethodInfo] = {}
        class_attrs: dict[str, ast.expr] = {}
        class_attr_values: dict[str, MoltValue] = {}
        class_annotation_items: list[tuple[str, MoltValue]] = []
        pending_methods: set[str] = {
            item.name
            for item in node.body
            if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef))
        }
        if len(base_names) != len(set(base_names)):
            dup = next(name for name in base_names if base_names.count(name) > 1)
            raise NotImplementedError(f"Duplicate base class {dup}")

        dynamic = dynamic_build or len(base_names) > 1
        if any(
            name not in self.classes
            and name not in BUILTIN_TYPE_TAGS
            and name not in BUILTIN_EXCEPTION_NAMES
            for name in base_names
        ):
            dynamic = True
        for name in base_names:
            base_info = self.classes.get(name)
            if base_info and base_info.get("dynamic"):
                dynamic = True
        if node.name in self.mutated_classes:
            dynamic = True
        # ``static`` marks a module-level top-level class whose class object has
        # a stable global-name binding, so its methods may reference the class
        # by module attribute (``_emit_class_ref`` -> module-attr-get) for layout
        # guards.  A *nested* ``class`` statement (``_class_body_depth > 0``) is
        # bound only into its enclosing class namespace — it has no module-global
        # name — so it must be treated exactly like a function-local class
        # (``current_func_name != "molt_main"``): non-static, routing its
        # methods' typed-field accesses through the instance-based generic path
        # rather than a non-existent module attribute.
        is_static = (
            self.current_func_name == "molt_main" and self._class_body_depth == 0
        )

        base_mros = [self._class_mro_names(name) for name in base_names]
        base_mros.append(list(base_names))
        merged = self._c3_merge(base_mros)
        if merged is None:
            merged = list(base_names)
        mro_names = [node.name] + merged

        if dataclass_opts is not None:
            for name in base_names:
                if name == "object":
                    continue
                base_info = self.classes.get(name)
                if base_info is None or not base_info.get("dataclass"):
                    # Non-dataclass bases are allowed; CPython permits
                    # inheriting from arbitrary classes in a @dataclass.
                    pass
            field_order: list[str] = []
            field_hints: dict[str, str] = {}
            for mro_name in mro_names[1:]:
                base_info = self.classes.get(mro_name)
                if base_info and base_info.get("dataclass"):
                    for name in base_info.get("field_order", []):
                        if name not in field_order:
                            field_order.append(name)

            def _annotation_kind(annotation: ast.AST) -> str | None:
                def _matches(expr: ast.AST, name: str) -> bool:
                    if isinstance(expr, ast.Name):
                        return expr.id == name
                    if isinstance(expr, ast.Attribute):
                        return expr.attr == name
                    return False

                if _matches(annotation, "KW_ONLY"):
                    return "kw_only"
                if isinstance(annotation, ast.Subscript):
                    if _matches(annotation.value, "ClassVar"):
                        return "classvar"
                    if _matches(annotation.value, "InitVar"):
                        return "initvar"
                if _matches(annotation, "ClassVar"):
                    return "classvar"
                if _matches(annotation, "InitVar"):
                    return "initvar"
                return None

            for item in node.body:
                if isinstance(item, ast.AnnAssign) and isinstance(
                    item.target, ast.Name
                ):
                    name = item.target.id
                    kind = _annotation_kind(item.annotation)
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
            field_indices = {name: idx for idx, name in enumerate(field_order)}
            min_layout = self._builtin_min_layout(mro_names)
            size = max(len(field_order) * 8, min_layout)
            repr_generated = dataclass_opts["repr"] and "__repr__" not in methods
            eq_generated = dataclass_opts["eq"] and "__eq__" not in methods
            self.classes[node.name] = {
                "fields": field_indices,
                "field_order": field_order,
                "field_hints": field_hints,
                "class_attrs": class_attrs,
                "module": self.module_name,
                "bases": base_names,
                "mro": mro_names,
                "dynamic": False,
                "static": is_static,
                "size": size,
                "dataclass": True,
                "frozen": dataclass_opts["frozen"],
                "eq": eq_generated,
                "repr": repr_generated,
                "slots": dataclass_opts["slots"],
                "dataclass_params": dataclass_opts,
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
            field_hints: dict[str, str] = {}
            for base_name in mro_names[1:]:
                base_info = self.classes.get(base_name)
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
                            # `__slots__` declares fixed instance field slots that
                            # the runtime's `apply_class_slots_layout` assigns real
                            # offsets to. Register each declared slot name as a
                            # field here so `class_info["size"]` reserves storage
                            # for it (slot_count * 8 + reserved_tail) — matching the
                            # runtime's `class_layout_size`. Omitting them made a
                            # `__slots__`-only class's frontend size disagree with
                            # the runtime's (e.g. a single slot: 8 vs 16), tripping
                            # the `alloc_instance_for_class_sized` layout-drift
                            # assert (and silently under-allocating in release).
                            if target.id == "__slots__":
                                for slot_name in _iter_slots_field_names(item.value):
                                    add_field(slot_name)

            methods_in_body = [
                item for item in node.body if isinstance(item, ast.FunctionDef)
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
                    if not _function_is_instance_method(method):
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

            min_layout = self._builtin_min_layout(mro_names)
            reserved_tail = self._class_reserved_tail_size(mro_names)
            base_size = (
                (len(field_order) * 8 + reserved_tail) if not dynamic else reserved_tail
            )
            size = max(base_size, min_layout)
            self.classes[node.name] = ClassInfo(
                fields=fields,
                size=size,
                methods=methods,
                pending_methods=pending_methods,
                field_order=field_order,
                defaults=field_defaults,
                field_hints=field_hints,
                class_attrs=class_attrs,
                module=self.module_name,
                bases=base_names,
                mro=mro_names,
                dynamic=dynamic,
                static=is_static,
                needs_classcell=needs_classcell,
                custom_metaclass=has_metaclass_kw
                or inherits_custom_meta
                or dynamic_build,
                decorated=bool(other_decorators),
            )

        method_names = {
            item.name
            for item in node.body
            if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef))
        }
        method_count = len(method_names)
        self.classes[node.name]["layout_version"] = self._class_layout_version(
            node.name,
            class_attrs,
            method_count=method_count,
        )

        def compile_generator_method(item: ast.FunctionDef) -> MethodInfo:
            descriptor: Literal[
                "function",
                "classmethod",
                "staticmethod",
                "property",
                "decorated",
                "property_update",
            ] = "function"
            method_name = item.name
            property_update: Literal["setter", "deleter"] | None = None
            if item.decorator_list:
                if len(item.decorator_list) == 1 and isinstance(
                    item.decorator_list[0], ast.Name
                ):
                    deco = item.decorator_list[0]
                    if deco.id in {"classmethod", "staticmethod", "property"}:
                        descriptor = cast(
                            Literal[
                                "function",
                                "classmethod",
                                "staticmethod",
                                "property",
                                "decorated",
                            ],
                            deco.id,
                        )
                    else:
                        descriptor = "decorated"
                elif len(item.decorator_list) == 1 and isinstance(
                    item.decorator_list[0], ast.Attribute
                ):
                    deco = item.decorator_list[0]
                    if (
                        isinstance(deco.value, ast.Name)
                        and deco.value.id == method_name
                        and deco.attr in {"setter", "deleter"}
                    ):
                        descriptor = "property_update"
                        property_update = cast(Literal["setter", "deleter"], deco.attr)
                    else:
                        descriptor = "decorated"
                else:
                    descriptor = "decorated"
            if descriptor == "function" and method_name == "__class_getitem__":
                descriptor = "classmethod"
            property_field = None
            if descriptor == "property":
                property_field = self._property_field_from_method(item)
            return_hint = self._annotation_to_hint(item.returns)
            if (
                return_hint
                and return_hint[:1] in {"'", '"'}
                and return_hint[-1:] == return_hint[:1]
            ):
                return_hint = return_hint[1:-1]
            if return_hint == "Self":
                return_hint = node.name
            method_symbol = self._function_symbol(f"{node.name}_{method_name}")
            self._record_func_default_specs(method_symbol, item.args)
            poll_symbol = f"{method_symbol}_poll"
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                item.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(item.args)
            default_specs = self._default_specs_from_args(item.args)
            arg_nodes: list[ast.arg] = posonly + pos_or_kw
            if item.args.vararg is not None:
                arg_nodes.append(item.args.vararg)
            arg_nodes.extend(kwonly)
            if item.args.kwarg is not None:
                arg_nodes.append(item.args.kwarg)
            free_vars, free_var_hints, closure_val, has_closure = (
                self._compute_method_closure(item)
            )
            has_return = self._function_contains_return(item)
            func_kind = "GenClosureFunc" if has_closure else "GenFunc"
            payload_slots = len(params) + (1 if has_closure else 0)
            closure_size = self._task_closure_size(
                payload_slots, include_gen_control=True
            )
            func_val = MoltValue(
                self.next_var(), type_hint=f"{func_kind}:{poll_symbol}:{closure_size}"
            )
            if has_closure and closure_val is not None:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW_CLOSURE",
                        args=[poll_symbol, len(params), closure_val],
                        result=func_val,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW",
                        args=[poll_symbol, len(params)],
                        result=func_val,
                    )
                )
            func_spill = None
            if self.in_generator and signature_contains_yield(
                decorators=item.decorator_list,
                args=item.args,
                returns=item.returns,
            ):
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )
            varnames = self._collect_varnames_for_body(
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                body=item.body,
            )
            self._emit_function_metadata(
                func_val,
                name=method_name,
                qualname=self._qualname_for_def(method_name),
                trace_lineno=item.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=[],
                kw_default_exprs=[],
                docstring=ast.get_docstring(item, clean=False),
                is_generator=True,
                varnames=varnames,
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, item)

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            prev_class = self.current_class
            prev_first_param = self.current_method_first_param
            self.current_class = node.name
            self.current_method_first_param = params[0] if params else None
            self.start_function(
                poll_symbol,
                params=["self"],
                type_facts_name=f"{node.name}.{method_name}",
                needs_return_slot=has_return,
            )
            self.global_decls = self._collect_global_decls(item.body)
            self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
            assigned = self._collect_assigned_names(item.body)
            self.del_targets = self._collect_deleted_names(item.body)
            self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
            self.unbound_check_names = set(self.scope_assigned)
            self.async_public_locals = set(self.scope_assigned) | {
                arg.arg for arg in arg_nodes
            }
            self.async_internal_locals = set()
            self.in_generator = True
            if has_closure:
                self.async_closure_offset = GEN_CONTROL_SIZE
                self.async_locals_base = GEN_CONTROL_SIZE + 8
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
            else:
                self.async_locals_base = GEN_CONTROL_SIZE
            for i, arg in enumerate(arg_nodes):
                self.async_locals[arg.arg] = self.async_locals_base + i * 8
                hint = None
                if i == 0 and descriptor == "classmethod":
                    hint = node.name
                elif i == 0 and descriptor not in ("classmethod", "staticmethod"):
                    hint = node.name
                if self._hints_enabled():
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                if hint is not None:
                    self.async_local_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            self._init_scope_async_locals(arg_nodes)
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            self._push_qualname(method_name, True)
            try:
                for stmt in item.body:
                    self.visit(stmt)
                    if isinstance(stmt, (ast.Return, ast.Raise)):
                        break
            finally:
                self._pop_qualname()
            if self.return_label is not None:
                if not self._ends_with_return_jump():
                    none_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                    closed = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", GEN_CLOSED_OFFSET, closed],
                            result=MoltValue("none"),
                        )
                    )
                    done = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
                    pair = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair)
                    )
                    self._emit_return_value(pair)
                self._emit_return_label()
            elif not (self.current_ops and self.current_ops[-1].kind == "ret"):
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                closed = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", GEN_CLOSED_OFFSET, closed],
                        result=MoltValue("none"),
                    )
                )
                done = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
                pair = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair))
                self.emit(MoltOp(kind="ret", args=[pair], result=MoltValue("none")))
            self._spill_async_temporaries()
            gen_public_locals = self._async_locals_public_entries()
            closure_size = self._task_closure_size(
                payload_slots, include_gen_control=True
            )
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            self.current_class = prev_class
            self.current_method_first_param = prev_first_param
            func_val.type_hint = f"{func_kind}:{poll_symbol}:{closure_size}"
            names_vals: list[MoltValue] = []
            offsets_vals: list[MoltValue] = []
            for local_name, offset in gen_public_locals:
                name_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[local_name], result=name_val))
                offset_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[offset], result=offset_val))
                names_vals.append(name_val)
                offsets_vals.append(offset_val)
            names_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=names_vals, result=names_tuple))
            offsets_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=offsets_vals, result=offsets_tuple))
            self.emit(
                MoltOp(
                    kind="GEN_LOCALS_REGISTER",
                    args=[poll_symbol, names_tuple, offsets_tuple],
                    result=MoltValue("none"),
                )
            )
            closure_size_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[closure_size], result=closure_size_val)
            )
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, "__molt_closure_size__", closure_size_val],
                    result=MoltValue("none"),
                )
            )
            method_attr = func_val
            return {
                "func": func_val,
                "attr": method_attr,
                "descriptor": descriptor,
                "return_hint": return_hint,
                "param_count": len(params),
                "defaults": default_specs,
                "posonly_count": len(posonly),
                "kwonly_count": len(kwonly),
                "has_vararg": vararg is not None,
                "has_varkw": varkw is not None,
                "has_closure": has_closure,
                "property_field": property_field,
                "property_update": property_update,
            }

        def compile_method(item: ast.FunctionDef) -> MethodInfo:
            descriptor: Literal[
                "function",
                "classmethod",
                "staticmethod",
                "property",
                "decorated",
                "property_update",
            ] = "function"
            method_name = item.name
            property_update: Literal["setter", "deleter"] | None = None
            if item.decorator_list:
                if len(item.decorator_list) == 1 and isinstance(
                    item.decorator_list[0], ast.Name
                ):
                    deco = item.decorator_list[0]
                    if deco.id in {"classmethod", "staticmethod", "property"}:
                        descriptor = cast(
                            Literal[
                                "function",
                                "classmethod",
                                "staticmethod",
                                "property",
                                "decorated",
                            ],
                            deco.id,
                        )
                    else:
                        descriptor = "decorated"
                elif len(item.decorator_list) == 1 and isinstance(
                    item.decorator_list[0], ast.Attribute
                ):
                    deco = item.decorator_list[0]
                    if (
                        isinstance(deco.value, ast.Name)
                        and deco.value.id == method_name
                        and deco.attr in {"setter", "deleter"}
                    ):
                        descriptor = "property_update"
                        property_update = cast(Literal["setter", "deleter"], deco.attr)
                    else:
                        descriptor = "decorated"
                else:
                    descriptor = "decorated"
            if descriptor == "function" and method_name == "__class_getitem__":
                descriptor = "classmethod"
            property_field = None
            if descriptor == "property":
                property_field = self._property_field_from_method(item)
            return_hint = self._annotation_to_hint(item.returns)
            if (
                return_hint
                and return_hint[:1] in {"'", '"'}
                and return_hint[-1:] == return_hint[:1]
            ):
                return_hint = return_hint[1:-1]
            if return_hint == "Self":
                return_hint = node.name
            method_symbol = self._function_symbol(f"{node.name}_{method_name}")
            self._record_func_default_specs(method_symbol, item.args)
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                item.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(item.args)
            default_specs = self._default_specs_from_args(item.args)
            free_vars, free_var_hints, closure_val, has_closure = (
                self._compute_method_closure(item)
            )

            func_hint = f"Func:{method_symbol}"
            if has_closure:
                func_hint = f"ClosureFunc:{method_symbol}"
            func_val = MoltValue(self.next_var(), type_hint=func_hint)
            if has_closure and closure_val is not None:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW_CLOSURE",
                        args=[method_symbol, len(params), closure_val],
                        result=func_val,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW",
                        args=[method_symbol, len(params)],
                        result=func_val,
                    )
                )
            func_spill = None
            if self.in_generator and signature_contains_yield(
                decorators=item.decorator_list,
                args=item.args,
                returns=item.returns,
            ):
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )
            varnames = self._collect_varnames_for_body(
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                body=item.body,
            )
            self._emit_function_metadata(
                func_val,
                name=method_name,
                qualname=self._qualname_for_def(method_name),
                trace_lineno=item.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=[],
                kw_default_exprs=[],
                docstring=ast.get_docstring(item, clean=False),
                varnames=varnames,
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, item)

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            prev_class = self.current_class
            prev_first_param = self.current_method_first_param
            self.current_class = node.name
            self.current_method_first_param = params[0] if params else None
            method_params = params
            if has_closure:
                method_params = [_MOLT_CLOSURE_PARAM] + params
            self.start_function(
                method_symbol,
                params=method_params,
                type_facts_name=f"{node.name}.{method_name}",
                needs_return_slot=False,
                has_exception_handlers=self._body_has_exception_handlers(item.body),
            )
            if has_closure:
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
                self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                    _MOLT_CLOSURE_PARAM, type_hint="tuple"
                )
            arg_nodes: list[ast.arg] = posonly + pos_or_kw
            if item.args.vararg is not None:
                arg_nodes.append(item.args.vararg)
            arg_nodes.extend(kwonly)
            if item.args.kwarg is not None:
                arg_nodes.append(item.args.kwarg)
            self.global_decls = self._collect_global_decls(item.body)
            self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
            assigned = self._collect_assigned_names(item.body)
            self.del_targets = self._collect_deleted_names(item.body)
            self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
            self.unbound_check_names = set(self.scope_assigned)
            for idx, arg in enumerate(arg_nodes):
                hint = None
                if idx == 0 and descriptor == "classmethod":
                    hint = node.name
                elif idx == 0 and descriptor not in ("classmethod", "staticmethod"):
                    hint = node.name
                if self._hints_enabled():
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                    elif hint is None:
                        hint = "Any"
                value = MoltValue(arg.arg, type_hint=hint or "Unknown")
                if hint is not None:
                    self._apply_hint_to_value(arg.arg, value, hint)
                self.locals[arg.arg] = value
            if self.type_hint_policy == "check":
                for arg in item.args.args:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(self.locals[arg.arg], hint)
            self._prebox_scope_cell_vars(body=item.body, arg_nodes=arg_nodes)
            # Box ALL scope-assigned variables into cell lists.
            # Cell lists provide correct refcount management (inc_ref/
            # dec_ref in molt_store_index). The TIR backend's Memory SSA
            # rewrite converts cell store_index/index to store_var/load_var
            # for SSA phi visibility when optimization is enabled.
            for name in sorted(self.scope_assigned):
                self._box_local(name)
            for arg in arg_nodes:
                pval = self.locals.get(arg.arg)
                if pval is not None and arg.arg not in self.boxed_locals:
                    self.emit(
                        MoltOp(
                            kind="STORE_VAR",
                            args=[pval],
                            result=MoltValue("none"),
                            metadata={"var": arg.arg},
                        )
                    )
            self._push_qualname(method_name, True)
            try:
                for stmt in item.body:
                    self.visit(stmt)
            finally:
                self._pop_qualname()
            if self.return_label is not None:
                if not self._ends_with_return_jump():
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                    self._emit_return_value(res)
                self._emit_return_label()
            elif not (self.current_ops and self.current_ops[-1].kind == "ret"):
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            self.current_class = prev_class
            self.current_method_first_param = prev_first_param
            method_attr = func_val
            # Phase 2 — detect trivially-inlinable methods.
            #
            # If the body is a single `return <expr>` where `<expr>`
            # references only parameters, attributes-on-parameters, and
            # constants (no Calls, no Subscripts, no closure / global
            # references), record the return-expression AST so the
            # call site can substitute params → args and emit the body
            # inline instead of a CALL.  Targets bench_class_hierarchy's
            # call chain (`Base.compute(self, x): return x`,
            # `Mid.compute(self, x): return ... + 1` — the latter is
            # filtered out because the AST still contains a Call to
            # super(); Phase 4a's fold runs at visit time, not at
            # compile_method-time AST inspection).
            inline_return = None
            inline_init_assigns: list[tuple[str, ast.expr]] | None = None
            inline_closure_ok = self._method_inline_closure_ok(free_vars, item)
            if (
                descriptor == "function"
                and inline_closure_ok
                and not (vararg is not None or varkw is not None)
                and not kwonly_names
            ):
                inline_return = self._extract_inline_return(item, params)
                # Detect __init__-style trivially-inlinable bodies — a
                # sequence of `self.attr = <pure expr>` assignments
                # where <pure expr> only references params/constants
                # and the targets are attributes of the first param.
                # The class-instantiation fold (visit_Call) inlines
                # these as a sequence of STORE_ATTR ops directly on
                # the freshly-allocated instance, eliminating the
                # __init__ CALL frame setup that dominates
                # bench_struct's per-iter cost.
                if method_name == "__init__":
                    inline_init_assigns = self._extract_inline_init_assigns(
                        item, params
                    )
            return {
                "func": func_val,
                "attr": method_attr,
                "descriptor": descriptor,
                "return_hint": return_hint,
                "param_count": len(params),
                "defaults": default_specs,
                "posonly_count": len(posonly),
                "kwonly_count": len(kwonly),
                "has_vararg": vararg is not None,
                "has_varkw": varkw is not None,
                "has_closure": has_closure,
                # True when ``has_closure`` is purely the implicit ``__class__``
                # super cell (no real enclosing-local capture).  The static
                # devirt / super-fold sites may then take the *inline* path
                # (whose recursive super-fold resolves the chain at compile
                # time and never reads the cell), but must NOT emit a direct
                # CALL to the closure symbol on inline failure — they fall back
                # to the general dispatch which threads the real closure tuple.
                "inline_closure_ok": inline_closure_ok,
                "property_field": property_field,
                "property_update": property_update,
                "inline_return": inline_return,
                "inline_params": (
                    params
                    if (inline_return is not None or inline_init_assigns is not None)
                    else None
                ),
                # Owner class — needed at inline time to set
                # `self.current_class` so that Phase 4a's `super()`
                # fold inside the inlined body resolves against the
                # callee's MRO position, not the caller's.
                "inline_owner_class": (
                    node.name
                    if (inline_return is not None or inline_init_assigns is not None)
                    else None
                ),
                # Module that defines this method, captured now (while compiling
                # the owner class) so the inline site can compare it against the
                # caller's module.  `inline_free_names` records the body's bare
                # references to that module's globals; a non-empty set forbids a
                # cross-module inline (the global would mis-resolve in the
                # caller's scope).  __init__-style inline-assign bodies carry the
                # same gate via the union over every assigned value-expression.
                "inline_owner_module": (
                    self.module_name
                    if (inline_return is not None or inline_init_assigns is not None)
                    else None
                ),
                "inline_free_names": (
                    self._inline_body_external_names(inline_return, params)
                    if inline_return is not None
                    else (
                        frozenset().union(
                            *(
                                self._inline_body_external_names(value_expr, params)
                                for _attr, value_expr in inline_init_assigns
                            )
                        )
                        if inline_init_assigns
                        else frozenset()
                    )
                ),
                "inline_init_assigns": inline_init_assigns,
            }

        def compile_async_method(item: ast.AsyncFunctionDef) -> MethodInfo:
            descriptor: Literal[
                "function",
                "classmethod",
                "staticmethod",
                "property",
                "decorated",
                "property_update",
            ] = "function"
            method_name = item.name
            property_update: Literal["setter", "deleter"] | None = None
            if item.decorator_list:
                if len(item.decorator_list) == 1 and isinstance(
                    item.decorator_list[0], ast.Name
                ):
                    deco = item.decorator_list[0]
                    if deco.id in {"classmethod", "staticmethod", "property"}:
                        descriptor = cast(
                            Literal[
                                "function",
                                "classmethod",
                                "staticmethod",
                                "property",
                                "decorated",
                            ],
                            deco.id,
                        )
                    else:
                        descriptor = "decorated"
                else:
                    descriptor = "decorated"
            if descriptor == "function" and method_name == "__class_getitem__":
                descriptor = "classmethod"
            is_async_gen = function_contains_yield(item)
            if is_async_gen:
                if async_generator_contains_yield_from(item):
                    raise SyntaxError("'yield from' inside async function")
                if async_generator_contains_return_value(item):
                    raise SyntaxError("'return' with value in async generator")
                method_name = item.name
                property_field = None
                return_hint = self._annotation_to_hint(item.returns)
                if (
                    return_hint
                    and return_hint[:1] in {"'", '"'}
                    and return_hint[-1:] == return_hint[:1]
                ):
                    return_hint = return_hint[1:-1]
                if return_hint == "Self":
                    return_hint = node.name
                wrapper_symbol = self._function_symbol(f"{node.name}_{method_name}")
                self._record_func_default_specs(wrapper_symbol, item.args)
                poll_symbol = f"{wrapper_symbol}_poll"
                posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                    item.args
                )
                posonly_names = [arg.arg for arg in posonly]
                pos_or_kw_names = [arg.arg for arg in pos_or_kw]
                kwonly_names = [arg.arg for arg in kwonly]
                params = self._function_param_names(item.args)
                arg_nodes: list[ast.arg] = posonly + pos_or_kw
                if item.args.vararg is not None:
                    arg_nodes.append(item.args.vararg)
                arg_nodes.extend(kwonly)
                if item.args.kwarg is not None:
                    arg_nodes.append(item.args.kwarg)
                default_specs = self._default_specs_from_args(item.args)
                free_vars, free_var_hints, closure_val, has_closure = (
                    self._compute_method_closure(item)
                )
                has_return = self._function_contains_return(item)

                prev_func = self.current_func_name
                prev_state = self._capture_function_state()
                prev_class = self.current_class
                prev_first_param = self.current_method_first_param
                self.current_class = node.name
                self.current_method_first_param = params[0] if params else None
                self.start_function(
                    poll_symbol,
                    params=["self"],
                    type_facts_name=f"{node.name}.{method_name}",
                    needs_return_slot=has_return,
                )
                self.async_context = True
                self.global_decls = self._collect_global_decls(item.body)
                self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
                assigned = self._collect_assigned_names(item.body)
                self.del_targets = self._collect_deleted_names(item.body)
                self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
                self.unbound_check_names = set(self.scope_assigned)
                self.async_public_locals = set(self.scope_assigned) | {
                    arg.arg for arg in arg_nodes
                }
                self.async_internal_locals = set()
                self.in_generator = True
                if has_closure:
                    self.async_closure_offset = GEN_CONTROL_SIZE
                    self.async_locals_base = GEN_CONTROL_SIZE + 8
                    self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                    self.free_var_hints = free_var_hints
                else:
                    self.async_locals_base = GEN_CONTROL_SIZE
                for i, arg in enumerate(arg_nodes):
                    self.async_locals[arg.arg] = self.async_locals_base + i * 8
                    hint = None
                    if i == 0 and descriptor == "classmethod":
                        hint = node.name
                    elif i == 0 and descriptor not in ("classmethod", "staticmethod"):
                        hint = node.name
                    if self._hints_enabled():
                        explicit = self.explicit_type_hints.get(arg.arg)
                        if explicit is None:
                            explicit = self._annotation_to_hint(arg.annotation)
                            if explicit is not None:
                                self.explicit_type_hints[arg.arg] = explicit
                        if explicit is not None:
                            hint = explicit
                    if hint is not None:
                        self.async_local_hints[arg.arg] = hint
                self._store_return_slot_for_stateful()
                self.emit(
                    MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none"))
                )
                self._init_scope_async_locals(arg_nodes)
                if self.type_hint_policy == "check":
                    for arg in arg_nodes:
                        hint = self.explicit_type_hints.get(arg.arg)
                        if hint is not None:
                            self._emit_guard_type(
                                MoltValue(arg.arg, type_hint=hint), hint
                            )
                self._push_qualname(method_name, True)
                try:
                    for stmt in item.body:
                        self.visit(stmt)
                        if isinstance(stmt, (ast.Return, ast.Raise)):
                            break
                finally:
                    self._pop_qualname()
                if self.return_label is not None:
                    if not self._ends_with_return_jump():
                        none_val = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                        closed = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
                        self.emit(
                            MoltOp(
                                kind="STORE_CLOSURE",
                                args=["self", GEN_CLOSED_OFFSET, closed],
                                result=MoltValue("none"),
                            )
                        )
                        done = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
                        pair = MoltValue(self.next_var(), type_hint="tuple")
                        self.emit(
                            MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair)
                        )
                        self._emit_return_value(pair)
                    self._emit_return_label()
                elif not (self.current_ops and self.current_ops[-1].kind == "ret"):
                    none_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                    closed = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", GEN_CLOSED_OFFSET, closed],
                            result=MoltValue("none"),
                        )
                    )
                    done = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
                    pair = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair)
                    )
                    self.emit(MoltOp(kind="ret", args=[pair], result=MoltValue("none")))
                self._spill_async_temporaries()
                asyncgen_public_locals = self._async_locals_public_entries()
                payload_slots = len(params) + (1 if has_closure else 0)
                closure_size = self._task_closure_size(
                    payload_slots, include_gen_control=True
                )
                self.resume_function(prev_func)
                self._restore_function_state(prev_state)
                self.current_class = prev_class
                self.current_method_first_param = prev_first_param

                func_hint = f"Func:{wrapper_symbol}"
                if has_closure:
                    func_hint = f"ClosureFunc:{wrapper_symbol}"
                func_val = MoltValue(self.next_var(), type_hint=func_hint)
                if has_closure and closure_val is not None:
                    self.emit(
                        MoltOp(
                            kind="FUNC_NEW_CLOSURE",
                            args=[wrapper_symbol, len(params), closure_val],
                            result=func_val,
                        )
                    )
                else:
                    self.emit(
                        MoltOp(
                            kind="FUNC_NEW",
                            args=[wrapper_symbol, len(params)],
                            result=func_val,
                        )
                    )
                func_spill = None
                if self.in_generator and signature_contains_yield(
                    decorators=item.decorator_list,
                    args=item.args,
                    returns=item.returns,
                ):
                    func_spill = self._spill_async_value(
                        func_val, f"__func_meta_{len(self.async_locals)}"
                    )
                varnames = self._collect_varnames_for_body(
                    posonly_params=posonly_names,
                    pos_or_kw_params=pos_or_kw_names,
                    kwonly_params=kwonly_names,
                    vararg=vararg,
                    varkw=varkw,
                    body=item.body,
                )
                self._emit_function_metadata(
                    func_val,
                    name=method_name,
                    qualname=self._qualname_for_def(method_name),
                    trace_lineno=item.lineno,
                    posonly_params=posonly_names,
                    pos_or_kw_params=pos_or_kw_names,
                    kwonly_params=kwonly_names,
                    vararg=vararg,
                    varkw=varkw,
                    default_exprs=[],
                    kw_default_exprs=[],
                    docstring=ast.get_docstring(item, clean=False),
                    is_async_generator=True,
                    poll_fn_symbol=poll_symbol,
                    varnames=varnames,
                )
                names_vals: list[MoltValue] = []
                offsets_vals: list[MoltValue] = []
                for local_name, offset in asyncgen_public_locals:
                    name_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(
                        MoltOp(kind="CONST_STR", args=[local_name], result=name_val)
                    )
                    offset_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[offset], result=offset_val))
                    names_vals.append(name_val)
                    offsets_vals.append(offset_val)
                names_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=names_vals, result=names_tuple))
                offsets_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(
                    MoltOp(kind="TUPLE_NEW", args=offsets_vals, result=offsets_tuple)
                )
                self.emit(
                    MoltOp(
                        kind="ASYNCGEN_LOCALS_REGISTER",
                        args=[poll_symbol, names_tuple, offsets_tuple],
                        result=MoltValue("none"),
                    )
                )
                if func_spill is not None:
                    func_val = self._reload_async_value(func_spill, func_val.type_hint)
                self._emit_function_annotate(func_val, item)
                closure_size_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(kind="CONST", args=[closure_size], result=closure_size_val)
                )
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[func_val, "__molt_closure_size__", closure_size_val],
                        result=MoltValue("none"),
                    )
                )

                prev_func = self.current_func_name
                prev_state = self._capture_function_state()
                wrapper_params = params
                if has_closure:
                    wrapper_params = [_MOLT_CLOSURE_PARAM] + params
                self.start_function(
                    wrapper_symbol,
                    params=wrapper_params,
                    type_facts_name=f"{node.name}.{method_name}",
                )
                if has_closure:
                    self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                        _MOLT_CLOSURE_PARAM, type_hint="tuple"
                    )
                self.global_decls = set()
                self.nonlocal_decls = set()
                self.scope_assigned = set()
                self.del_targets = set()
                for idx, arg in enumerate(arg_nodes):
                    hint = None
                    if idx == 0 and descriptor == "classmethod":
                        hint = node.name
                    elif idx == 0 and descriptor not in ("classmethod", "staticmethod"):
                        hint = node.name
                    if self._hints_enabled():
                        explicit = self.explicit_type_hints.get(arg.arg)
                        if explicit is None:
                            explicit = self._annotation_to_hint(arg.annotation)
                            if explicit is not None:
                                self.explicit_type_hints[arg.arg] = explicit
                        if explicit is not None:
                            hint = explicit
                        elif hint is None:
                            hint = "Any"
                    value = MoltValue(arg.arg, type_hint=hint or "Unknown")
                    if hint is not None:
                        self._apply_hint_to_value(arg.arg, value, hint)
                    self.locals[arg.arg] = value
                if self.type_hint_policy == "check":
                    for arg in arg_nodes:
                        hint = self.explicit_type_hints.get(arg.arg)
                        if hint is not None:
                            self._emit_guard_type(self.locals[arg.arg], hint)
                args = [self.locals[arg.arg] for arg in arg_nodes]
                if has_closure:
                    args = [self.locals[_MOLT_CLOSURE_PARAM]] + args
                gen_val = MoltValue(self.next_var(), type_hint="generator")
                self.emit(
                    MoltOp(
                        kind="ALLOC_TASK",
                        args=[poll_symbol, closure_size] + args,
                        result=gen_val,
                        metadata={"task_kind": "generator"},
                    )
                )
                res = MoltValue(self.next_var(), type_hint="async_generator")
                self.emit(MoltOp(kind="ASYNCGEN_NEW", args=[gen_val], result=res))
                self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
                self.resume_function(prev_func)
                self._restore_function_state(prev_state)

                method_attr = func_val
                return {
                    "func": func_val,
                    "attr": method_attr,
                    "descriptor": descriptor,
                    "return_hint": return_hint,
                    "param_count": len(params),
                    "defaults": default_specs,
                    "posonly_count": len(posonly),
                    "kwonly_count": len(kwonly),
                    "has_vararg": vararg is not None,
                    "has_varkw": varkw is not None,
                    "has_closure": has_closure,
                    "property_field": property_field,
                    "property_update": property_update,
                }
            method_name = item.name
            property_field = None
            return_hint = self._annotation_to_hint(item.returns)
            if (
                return_hint
                and return_hint[:1] in {"'", '"'}
                and return_hint[-1:] == return_hint[:1]
            ):
                return_hint = return_hint[1:-1]
            if return_hint == "Self":
                return_hint = node.name
            wrapper_symbol = self._function_symbol(f"{node.name}_{method_name}")
            self._record_func_default_specs(wrapper_symbol, item.args)
            poll_symbol = f"{wrapper_symbol}_poll"
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                item.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(item.args)
            arg_nodes: list[ast.arg] = posonly + pos_or_kw
            if item.args.vararg is not None:
                arg_nodes.append(item.args.vararg)
            arg_nodes.extend(kwonly)
            if item.args.kwarg is not None:
                arg_nodes.append(item.args.kwarg)
            default_specs = self._default_specs_from_args(item.args)
            free_vars, free_var_hints, closure_val, has_closure = (
                self._compute_method_closure(item)
            )
            has_return = self._function_contains_return(item)

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            prev_class = self.current_class
            prev_first_param = self.current_method_first_param
            self.current_class = node.name
            self.current_method_first_param = params[0] if params else None
            self.start_function(
                poll_symbol,
                params=["self"],
                type_facts_name=f"{node.name}.{method_name}",
                needs_return_slot=has_return,
            )
            self.async_context = True
            self.global_decls = self._collect_global_decls(item.body)
            self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
            assigned = self._collect_assigned_names(item.body)
            self.del_targets = self._collect_deleted_names(item.body)
            self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
            self.unbound_check_names = set(self.scope_assigned)
            if has_closure:
                self.async_closure_offset = 0
                self.async_locals_base = 8
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
            for i, arg in enumerate(arg_nodes):
                self.async_locals[arg.arg] = self.async_locals_base + i * 8
                hint = None
                if i == 0 and descriptor == "classmethod":
                    hint = node.name
                elif i == 0 and descriptor not in ("classmethod", "staticmethod"):
                    hint = node.name
                if self._hints_enabled():
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                if hint is not None:
                    self.async_local_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            self._init_scope_async_locals(arg_nodes)
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            self._push_qualname(method_name, True)
            try:
                for stmt in item.body:
                    self.visit(stmt)
            finally:
                self._pop_qualname()
            if self.return_label is not None:
                if not self._ends_with_return_jump():
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                    self._emit_return_value(res)
                self._emit_return_label()
            else:
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
            self._spill_async_temporaries()
            payload_slots = len(params) + (1 if has_closure else 0)
            closure_size = self._task_closure_size(
                payload_slots, include_gen_control=False
            )
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            self.current_class = prev_class
            self.current_method_first_param = prev_first_param

            func_hint = f"Func:{wrapper_symbol}"
            if has_closure:
                func_hint = f"ClosureFunc:{wrapper_symbol}"
            func_val = MoltValue(self.next_var(), type_hint=func_hint)
            if has_closure and closure_val is not None:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW_CLOSURE",
                        args=[wrapper_symbol, len(params), closure_val],
                        result=func_val,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW",
                        args=[wrapper_symbol, len(params)],
                        result=func_val,
                    )
                )
            func_spill = None
            if self.in_generator and signature_contains_yield(
                decorators=item.decorator_list,
                args=item.args,
                returns=item.returns,
            ):
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )
            varnames = self._collect_varnames_for_body(
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                body=item.body,
            )
            self._emit_function_metadata(
                func_val,
                name=method_name,
                qualname=self._qualname_for_def(method_name),
                trace_lineno=item.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=[],
                kw_default_exprs=[],
                docstring=ast.get_docstring(item, clean=False),
                is_coroutine=True,
                varnames=varnames,
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, item)
            closure_size_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[closure_size], result=closure_size_val)
            )
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, "__molt_closure_size__", closure_size_val],
                    result=MoltValue("none"),
                )
            )

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            wrapper_params = params
            if has_closure:
                wrapper_params = [_MOLT_CLOSURE_PARAM] + params
            self.start_function(
                wrapper_symbol,
                params=wrapper_params,
                type_facts_name=f"{node.name}.{method_name}",
            )
            if has_closure:
                self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                    _MOLT_CLOSURE_PARAM, type_hint="tuple"
                )
            self.global_decls = set()
            self.nonlocal_decls = set()
            self.scope_assigned = set()
            self.del_targets = set()
            for idx, arg in enumerate(arg_nodes):
                hint = None
                if idx == 0 and descriptor == "classmethod":
                    hint = node.name
                elif idx == 0 and descriptor not in ("classmethod", "staticmethod"):
                    hint = node.name
                if self._hints_enabled():
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                    elif hint is None:
                        hint = "Any"
                value = MoltValue(arg.arg, type_hint=hint or "Unknown")
                if hint is not None:
                    self._apply_hint_to_value(arg.arg, value, hint)
                self.locals[arg.arg] = value
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(self.locals[arg.arg], hint)
            args = [self.locals[arg.arg] for arg in arg_nodes]
            if has_closure:
                args = [self.locals[_MOLT_CLOSURE_PARAM]] + args
            res = MoltValue(self.next_var(), type_hint="Future")
            self.emit(
                MoltOp(
                    kind="ALLOC_TASK",
                    args=[poll_symbol, closure_size] + args,
                    result=res,
                    metadata={"task_kind": "coroutine"},
                )
            )
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)

            method_attr = func_val
            return {
                "func": func_val,
                "attr": method_attr,
                "descriptor": descriptor,
                "return_hint": return_hint,
                "param_count": len(params),
                "defaults": default_specs,
                "posonly_count": len(posonly),
                "kwonly_count": len(kwonly),
                "has_vararg": vararg is not None,
                "has_varkw": varkw is not None,
                "has_closure": has_closure,
                "property_field": property_field,
                "property_update": property_update,
            }

        # ``__class__`` cell — created BEFORE compiling methods so it can be
        # threaded into each method's closure as the implicit ``__class__``
        # free variable (CPython semantics).  A cell is a 1-element list whose
        # slot is filled with the finished class object after the class is
        # built (see the cell-fill emission on both the dynamic and outlined
        # paths below).  Zero-arg ``super()`` and bare ``__class__`` loads read
        # ``cell[0]`` from the closure, so they resolve correctly for
        # function-local, nested, and module-level classes (including
        # metaclasses) uniformly — instead of re-deriving the class by
        # module-attribute name, which fails when the class is not a module
        # global.
        classcell_val: MoltValue | None = None
        prev_active_classcell = self._active_classcell_cell
        prev_classcell_boxed = self.boxed_locals.get("__class__")
        prev_classcell_hint = self.boxed_local_hints.get("__class__")
        prev_classcell_locals = self.locals.get("__class__")
        if needs_classcell:
            none_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
            classcell_val = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[none_val], result=classcell_val))
            self._active_classcell_cell = classcell_val
            self.boxed_locals["__class__"] = classcell_val
            self.boxed_local_hints["__class__"] = "type"
            self.locals["__class__"] = classcell_val

        self._push_qualname(node.name, False)
        try:
            for item in node.body:
                if isinstance(item, ast.FunctionDef):
                    if function_contains_yield(item):
                        method_info = compile_generator_method(item)
                    else:
                        method_info = compile_method(item)
                    if method_info["descriptor"] == "property_update":
                        property_updates[id(item)] = method_info
                    else:
                        methods[item.name] = method_info
                elif isinstance(item, ast.AsyncFunctionDef):
                    method_info = compile_async_method(item)
                    if method_info["descriptor"] == "property_update":
                        property_updates[id(item)] = method_info
                    else:
                        methods[item.name] = method_info
        finally:
            self._pop_qualname()
            # Restore the enclosing scope's view of ``__class__`` now that the
            # methods have been compiled: ``__class__`` is only an implicit
            # closure variable inside the class body, never a real local of the
            # surrounding function.  The cell MoltValue (``classcell_val``)
            # remains live and is filled with the finished class object below.
            self._active_classcell_cell = prev_active_classcell
            if prev_classcell_boxed is None:
                self.boxed_locals.pop("__class__", None)
            else:
                self.boxed_locals["__class__"] = prev_classcell_boxed
            if prev_classcell_hint is None:
                self.boxed_local_hints.pop("__class__", None)
            else:
                self.boxed_local_hints["__class__"] = prev_classcell_hint
            if prev_classcell_locals is None:
                self.locals.pop("__class__", None)
            else:
                self.locals["__class__"] = prev_classcell_locals

        layout_version = self._class_layout_version(
            node.name, class_attrs, methods=methods
        )
        prior_layout = self.classes[node.name].get("layout_version")
        if prior_layout is not None and prior_layout != layout_version:
            raise RuntimeError(
                "Class layout version changed after method compilation for "
                f"{node.name}: pre={prior_layout} post={layout_version}"
            )
        self.classes[node.name]["layout_version"] = layout_version

        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[node.name], result=name_val))
        qualname = self._qualname_for_def(node.name)
        qualname_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[qualname], result=qualname_val))
        module_name = (
            "__main__"
            if self.entry_module and self.module_name == self.entry_module
            else self.module_name
        )
        module_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=module_val))

        dynamic_namespace: MoltValue | None = None
        # ``classcell_val`` is created earlier (before the method loop) when
        # ``needs_classcell`` so it can be threaded into method closures; do not
        # re-declare it here or the pre-created cell would be lost.
        dynamic_bases_tuple: MoltValue | None = None
        dynamic_meta: MoltValue | None = None
        dynamic_prepared_kwds: MoltValue | None = None
        dynamic_kw_pairs: list[tuple[str, MoltValue]] = []
        dynamic_kw_splats: list[MoltValue] = []
        if dynamic_build:
            for kw in node.keywords:
                if kw.arg is None:
                    splat_val = self.visit(kw.value)
                    if splat_val is None:
                        raise NotImplementedError("Unsupported class **kwargs value")
                    dynamic_kw_splats.append(splat_val)
                    continue
                kw_val = self.visit(kw.value)
                if kw_val is None:
                    raise NotImplementedError("Unsupported class keyword value")
                dynamic_kw_pairs.append((kw.arg, kw_val))

            if has_explicit_bases:
                bases_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=base_vals, result=bases_tuple))
                dynamic_bases_tuple = bases_tuple
            else:
                empty_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=empty_tuple))
                dynamic_bases_tuple = empty_tuple

            if dynamic_bases_tuple is None:
                raise NotImplementedError("Unsupported class bases")
            types_bootstrap_func = self._emit_intrinsic_function("molt_types_bootstrap")
            types_bootstrap = self._emit_call_bound_or_func(types_bootstrap_func, [])
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
            kwds_val = none_val
            if dynamic_kw_pairs or dynamic_kw_splats:
                kwds_dict = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_NEW", args=[], result=kwds_dict))
                for kw_name, kw_val in dynamic_kw_pairs:
                    key_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[kw_name], result=key_val))
                    self.emit(
                        MoltOp(
                            kind="STORE_INDEX",
                            args=[kwds_dict, key_val, kw_val],
                            result=MoltValue("none"),
                        )
                    )
                for splat_val in dynamic_kw_splats:
                    self.emit(
                        MoltOp(
                            kind="DICT_UPDATE_KWSTAR",
                            args=[kwds_dict, splat_val],
                            result=MoltValue("none"),
                        )
                    )
                kwds_val = kwds_dict

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

            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=["__module__"], result=key_val))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[namespace_val, key_val, module_val],
                    result=MoltValue("none"),
                )
            )
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=["__qualname__"], result=key_val))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[namespace_val, key_val, qualname_val],
                    result=MoltValue("none"),
                )
            )
            # __firstlineno__ (CPython 3.13+) — line number of the class statement
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="CONST_STR", args=["__firstlineno__"], result=key_val)
            )
            lineno_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[node.lineno], result=lineno_val))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[namespace_val, key_val, lineno_val],
                    result=MoltValue("none"),
                )
            )
            dynamic_namespace = namespace_val
            if needs_classcell and classcell_val is not None:
                # Reuse the cell created before the method loop (the same cell
                # threaded into method closures), and publish it under
                # ``__classcell__`` so the metaclass's ``type.__new__`` fills it
                # with the finished class — exactly as CPython does.
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(kind="CONST_STR", args=["__classcell__"], result=key_val)
                )
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[dynamic_namespace, key_val, classcell_val],
                        result=MoltValue("none"),
                    )
                )

        class_scope: dict[str, MoltValue] = {
            name: info["attr"] for name, info in methods.items()
        }
        saved_locals = self.locals
        self.locals = dict(class_scope)
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
            names=set(class_attr_values) | set(methods),
        )

        def bind_class_name(name: str, value: MoltValue) -> None:
            # Single source of truth for "this straight-line arm bound a
            # class-body attribute": update the SSA fast-path view, mirror into
            # the namespace dict (when present), and keep the enclosing-frame
            # ``self.locals`` cache coherent for the rare in-body load that
            # predates control flow.
            self._class_ns_store(class_ns_scope, name, value)
            self.locals[name] = value

        # The class-ns scope is pushed onto the stack ONLY for bodies that need
        # block execution (control flow / ``del``).  A straight-line body keeps
        # the original fast path: its name binds go through ``bind_class_name``
        # (which updates ``class_attr_values`` / ``self.locals`` and, for a
        # dynamic class, the namespace dict) but the ``_store_local_value`` /
        # ``_load_local_value`` / ``_emit_delete_name`` hooks stay INERT (an
        # empty stack), so emission of field defaults, method defaults, and the
        # compile-time dataclass path is byte-for-byte unchanged.  (P0 #50.)
        _push_scope = body_needs_block
        if _push_scope:
            self._class_ns_stack.append(class_ns_scope)
        try:
            for item in node.body:
                if isinstance(item, ast.ClassDef):
                    # A nested ``class`` statement.  Lower it recursively: this
                    # emits the nested class's own ``CLASS_DEF`` (so the class
                    # object exists before it is attached to the enclosing
                    # class) and — because ``self._class_body_depth > 0`` — binds
                    # it into ``self.locals`` rather than module globals.  Harvest
                    # that binding into the enclosing class namespace, mirroring
                    # the plain class-attribute ``Assign`` path below so methods
                    # referencing the nested class by name resolve and the class
                    # object is published as ``Enclosing.Nested``.
                    self.visit_ClassDef(item)
                    nested_val = self.locals.get(item.name)
                    if nested_val is None:
                        raise NotImplementedError(
                            "Nested class lowering produced no bound value for "
                            f"'{item.name}'"
                        )
                    bind_class_name(item.name, nested_val)
                    continue
                if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    _, _, kwonly, _, _ = self._split_function_args(item.args)
                    kwonly_names = [arg.arg for arg in kwonly]
                    update_info = property_updates.get(id(item))
                    if update_info is not None:
                        update_info["func"] = self._emit_function_defaults(
                            update_info["func"],
                            item.args.defaults,
                            item.args.kw_defaults,
                            kwonly_names,
                        )
                        update_kind = update_info.get("property_update")
                        if update_kind is None:
                            raise NotImplementedError("Property update kind missing")
                        prop_val = class_scope.get(item.name)
                        if prop_val is None:
                            exc_val = self._emit_exception_new(
                                "NameError", f"name '{item.name}' is not defined"
                            )
                            self.emit(
                                MoltOp(
                                    kind="RAISE",
                                    args=[exc_val],
                                    result=MoltValue("none"),
                                )
                            )
                            prop_val = MoltValue(self.next_var(), type_hint="None")
                            self.emit(
                                MoltOp(kind="CONST_NONE", args=[], result=prop_val)
                            )
                        prop_attr = MoltValue(self.next_var(), type_hint="Any")
                        self.emit(
                            MoltOp(
                                kind="GETATTR_GENERIC_PTR",
                                args=[prop_val, update_kind],
                                result=prop_attr,
                                metadata={"ic_index": _next_ic_index()},
                            )
                        )
                        callargs = MoltValue(self.next_var(), type_hint="callargs")
                        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
                        self.emit(
                            MoltOp(
                                kind="CALLARGS_PUSH_POS",
                                args=[callargs, update_info["attr"]],
                                result=MoltValue("none"),
                            )
                        )
                        res = MoltValue(self.next_var(), type_hint="property")
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND",
                                args=[prop_attr, callargs],
                                result=res,
                            )
                        )
                        class_scope[item.name] = res
                        self.locals[item.name] = res
                        # A method/descriptor name is a class-body binding too:
                        # register it so in-body loads (e.g. a later
                        # ``@name.setter``) resolve via the class namespace
                        # (P0 #50).  Methods publish into the build via
                        # ``methods``/``class_attr_values`` below, not via
                        # ``_class_ns_store``, so only the name set is updated.
                        class_ns_scope.names.add(item.name)
                        # Keep the canonical method binding in sync so later
                        # class finalization does not overwrite descriptor
                        # updates (e.g. @name.setter / @name.deleter) with the
                        # original pre-update descriptor value.
                        if item.name in methods:
                            methods[item.name]["attr"] = res
                        else:
                            class_attr_values[item.name] = res
                        if dynamic_namespace is not None:
                            key_val = MoltValue(self.next_var(), type_hint="str")
                            self.emit(
                                MoltOp(
                                    kind="CONST_STR", args=[item.name], result=key_val
                                )
                            )
                            self.emit(
                                MoltOp(
                                    kind="STORE_INDEX",
                                    args=[dynamic_namespace, key_val, res],
                                    result=MoltValue("none"),
                                )
                            )
                        continue
                    method_info = methods.get(item.name)
                    if method_info is not None:
                        func_val = method_info["func"]
                        func_val = self._emit_function_defaults(
                            func_val,
                            item.args.defaults,
                            item.args.kw_defaults,
                            kwonly_names,
                        )
                        method_attr = func_val
                        descriptor = method_info["descriptor"]
                        if descriptor == "decorated":
                            property_outer = (
                                item.decorator_list
                                and isinstance(item.decorator_list[0], ast.Name)
                                and item.decorator_list[0].id == "property"
                            )
                            if property_outer:
                                method_decorator_vals: list[MoltValue] = []
                                for deco in item.decorator_list[1:]:
                                    decorator_val = self.visit(deco)
                                    if decorator_val is None:
                                        raise NotImplementedError(
                                            "Unsupported method decorator"
                                        )
                                    method_decorator_vals.append(decorator_val)
                                decorated = method_attr
                                for decorator_val in reversed(method_decorator_vals):
                                    callargs = MoltValue(
                                        self.next_var(), type_hint="callargs"
                                    )
                                    self.emit(
                                        MoltOp(
                                            kind="CALLARGS_NEW",
                                            args=[],
                                            result=callargs,
                                        )
                                    )
                                    push_res = MoltValue(
                                        self.next_var(), type_hint="None"
                                    )
                                    self.emit(
                                        MoltOp(
                                            kind="CALLARGS_PUSH_POS",
                                            args=[callargs, decorated],
                                            result=push_res,
                                        )
                                    )
                                    res = MoltValue(self.next_var(), type_hint="Any")
                                    self.emit(
                                        MoltOp(
                                            kind="CALL_BIND",
                                            args=[decorator_val, callargs],
                                            result=res,
                                        )
                                    )
                                    decorated = res
                                none_val = MoltValue(self.next_var(), type_hint="None")
                                self.emit(
                                    MoltOp(kind="CONST_NONE", args=[], result=none_val)
                                )
                                wrapped = MoltValue(
                                    self.next_var(), type_hint="property"
                                )
                                self.emit(
                                    MoltOp(
                                        kind="PROPERTY_NEW",
                                        args=[decorated, none_val, none_val],
                                        result=wrapped,
                                    )
                                )
                                method_attr = wrapped
                            else:
                                method_decorator_vals = []
                                for deco in item.decorator_list:
                                    decorator_val = self.visit(deco)
                                    if decorator_val is None:
                                        raise NotImplementedError(
                                            "Unsupported method decorator"
                                        )
                                    method_decorator_vals.append(decorator_val)
                                decorated = method_attr
                                for decorator_val in reversed(method_decorator_vals):
                                    callargs = MoltValue(
                                        self.next_var(), type_hint="callargs"
                                    )
                                    self.emit(
                                        MoltOp(
                                            kind="CALLARGS_NEW",
                                            args=[],
                                            result=callargs,
                                        )
                                    )
                                    push_res = MoltValue(
                                        self.next_var(), type_hint="None"
                                    )
                                    self.emit(
                                        MoltOp(
                                            kind="CALLARGS_PUSH_POS",
                                            args=[callargs, decorated],
                                            result=push_res,
                                        )
                                    )
                                    res = MoltValue(self.next_var(), type_hint="Any")
                                    self.emit(
                                        MoltOp(
                                            kind="CALL_BIND",
                                            args=[decorator_val, callargs],
                                            result=res,
                                        )
                                    )
                                    decorated = res
                                method_attr = decorated
                        elif descriptor == "classmethod":
                            wrapped = MoltValue(
                                self.next_var(), type_hint="classmethod"
                            )
                            self.emit(
                                MoltOp(
                                    kind="CLASSMETHOD_NEW",
                                    args=[func_val],
                                    result=wrapped,
                                )
                            )
                            method_attr = wrapped
                        elif descriptor == "staticmethod":
                            wrapped = MoltValue(
                                self.next_var(), type_hint="staticmethod"
                            )
                            self.emit(
                                MoltOp(
                                    kind="STATICMETHOD_NEW",
                                    args=[func_val],
                                    result=wrapped,
                                )
                            )
                            method_attr = wrapped
                        elif descriptor == "property":
                            none_val = MoltValue(self.next_var(), type_hint="None")
                            self.emit(
                                MoltOp(kind="CONST_NONE", args=[], result=none_val)
                            )
                            wrapped = MoltValue(self.next_var(), type_hint="property")
                            self.emit(
                                MoltOp(
                                    kind="PROPERTY_NEW",
                                    args=[func_val, none_val, none_val],
                                    result=wrapped,
                                )
                            )
                            method_attr = wrapped
                        method_info["attr"] = method_attr
                        class_scope[item.name] = method_attr
                        self.locals[item.name] = method_attr
                        # Register the method name as a class-body binding so an
                        # in-body load resolves through the class namespace
                        # (P0 #50); the build consumes ``methods`` directly.
                        class_ns_scope.names.add(item.name)
                        if dynamic_namespace is not None:
                            key_val = MoltValue(self.next_var(), type_hint="str")
                            self.emit(
                                MoltOp(
                                    kind="CONST_STR", args=[item.name], result=key_val
                                )
                            )
                            self.emit(
                                MoltOp(
                                    kind="STORE_INDEX",
                                    args=[
                                        dynamic_namespace,
                                        key_val,
                                        method_attr,
                                    ],
                                    result=MoltValue("none"),
                                )
                            )
                    continue
                if isinstance(item, ast.Expr):
                    if isinstance(item.value, ast.Constant) and isinstance(
                        item.value.value, str
                    ):
                        continue
                    if self.visit(item.value) is None:
                        raise NotImplementedError("Unsupported class body expression")
                    continue
                if isinstance(item, ast.Assign) and all(
                    isinstance(t, ast.Name) for t in item.targets
                ):
                    val = self.visit(item.value)
                    if val is None:
                        raise NotImplementedError("Unsupported class body assignment")
                    for target in item.targets:
                        assert isinstance(target, ast.Name)
                        bind_class_name(target.id, val)
                    continue
                if isinstance(item, ast.AnnAssign) and isinstance(
                    item.target, ast.Name
                ):
                    if self.future_annotations or self.eager_annotations:
                        ann_val = self._emit_annotation_value(
                            item.annotation, stringize=self.future_annotations
                        )
                        class_annotation_items.append((item.target.id, ann_val))
                    else:
                        exec_map = self._ensure_class_annotation_exec_map(node.name)
                        exec_id = self._annotation_exec_id(is_module=False)
                        self._emit_annotation_exec_mark(exec_map, exec_id)
                        self.class_annotation_items.append(
                            (item.target.id, item.annotation, exec_id)
                        )
                    if item.value is None:
                        continue
                    val = self.visit(item.value)
                    if val is None:
                        raise NotImplementedError("Unsupported class body assignment")
                    bind_class_name(item.target.id, val)
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
        finally:
            self._class_body_depth -= 1
            self.locals = saved_locals
            if _push_scope:
                popped_scope = self._class_ns_stack.pop()
                assert popped_scope is class_ns_scope, "class-ns scope stack imbalance"

        # __static_attributes__ (CPython 3.13+) — always emitted after class
        # body, even when empty.  Appears after methods in namespace event order.
        classdictcell_key: MoltValue | None = None
        if dynamic_namespace is not None:
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
                MoltOp(kind="CONST_STR", args=["__static_attributes__"], result=key_val)
            )
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[dynamic_namespace, key_val, static_tuple],
                    result=MoltValue("none"),
                )
            )
            # __classdictcell__ (CPython 3.14+) — the class body dict cell,
            # set when the class has methods.  This is NOT the same as
            # __classcell__ (which is for super() support).
            has_methods = any(
                isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef))
                for item in node.body
            )
            if has_methods:
                # The value is the namespace dict itself (class body dict cell)
                cdc_key = MoltValue(self.next_var(), type_hint="str")
                classdictcell_key = cdc_key
                self.emit(
                    MoltOp(kind="CONST_STR", args=["__classdictcell__"], result=cdc_key)
                )
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[dynamic_namespace, cdc_key, dynamic_namespace],
                        result=MoltValue("none"),
                    )
                )

        if (
            (self.future_annotations or self.eager_annotations)
            and dynamic_namespace is not None
            and class_annotation_items
            and "__annotations__" not in class_attr_values
        ):
            ann_items: list[MoltValue] = []
            for name, val in class_annotation_items:
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                ann_items.extend([key_val, val])
            ann_dict = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=ann_items, result=ann_dict))
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="CONST_STR", args=["__annotations__"], result=key_val)
            )
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[dynamic_namespace, key_val, ann_dict],
                    result=MoltValue("none"),
                )
            )
        elif (
            not self.future_annotations
            and not self.eager_annotations
            and dynamic_namespace is not None
            and self.class_annotation_items
            and "__annotations__" not in class_attr_values
        ):
            # PEP 749 deferred annotations: the __annotate__ function is
            # emitted separately, but CPython 3.14 also needs __annotations__
            # eagerly accessible on class objects (via type descriptor).
            # Since our runtime doesn't implement the type.__annotations__
            # descriptor, eagerly evaluate and store annotations here.
            ann_items: list[MoltValue] = []
            for name, expr, _exec_id in self.class_annotation_items:
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                ann_val = self._emit_annotation_value(expr, stringize=False)
                ann_items.extend([key_val, ann_val])
            ann_dict = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=ann_items, result=ann_dict))
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="CONST_STR", args=["__annotations__"], result=key_val)
            )
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[dynamic_namespace, key_val, ann_dict],
                    result=MoltValue("none"),
                )
            )

        if dynamic_build:
            if (
                dynamic_meta is None
                or dynamic_bases_tuple is None
                or dynamic_namespace is None
            ):
                raise NotImplementedError("Unsupported dynamic class build")
            if classdictcell_key is not None:
                self.emit(
                    MoltOp(
                        kind="DEL_INDEX",
                        args=[dynamic_namespace, classdictcell_key],
                        result=MoltValue("none"),
                    )
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
            class_val = MoltValue(self.next_var(), type_hint="type")
            self.emit(
                MoltOp(
                    kind="CALL_BIND", args=[dynamic_meta, callargs], result=class_val
                )
            )
            if needs_classcell and classcell_val is not None:
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(kind="CONST_STR", args=["__classcell__"], result=key_val)
                )
                cell_val = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="DICT_GET",
                        args=[dynamic_namespace, key_val, none_val],
                        result=cell_val,
                    )
                )
                is_missing = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(kind="IS", args=[cell_val, none_val], result=is_missing)
                )
                self.emit(
                    MoltOp(kind="IF", args=[is_missing], result=MoltValue("none"))
                )
                msg = (
                    "__class__ not set defining "
                    f"'{node.name}' as <class '{module_name}.{node.name}'>. "
                    "Was __classcell__ propagated to type.__new__?"
                )
                msg_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[msg], result=msg_val))
                exc_val = self._emit_exception_new("RuntimeError", msg_val)
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
                self._emit_raise_exit()
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                same_cell = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(kind="IS", args=[cell_val, classcell_val], result=same_cell)
                )
                self.emit(MoltOp(kind="IF", args=[same_cell], result=MoltValue("none")))
                zero_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=zero_val))
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[classcell_val, zero_val, class_val],
                        result=MoltValue("none"),
                    )
                )
                self.emit(
                    MoltOp(
                        kind="DEL_INDEX",
                        args=[dynamic_namespace, key_val],
                        result=MoltValue("none"),
                    )
                )
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                msg_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[msg], result=msg_val))
                exc_val = self._emit_exception_new("RuntimeError", msg_val)
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
                self._emit_raise_exit()
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        else:
            # Outlined class definition: collect attrs, emit single CLASS_DEF op
            class_def_attrs: list[tuple[MoltValue, MoltValue]] = []
            # __firstlineno__ (CPython 3.13+)
            lineno_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[node.lineno], result=lineno_val))
            for attr_str, attr_val in [
                ("__name__", name_val),
                ("__qualname__", qualname_val),
                ("__module__", module_val),
                ("__firstlineno__", lineno_val),
            ]:
                key = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[attr_str], result=key))
                class_def_attrs.append((key, attr_val))
            # __static_attributes__ (CPython 3.13+)
            static_attrs = self._collect_static_attributes(node)
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

        class_info = self.classes[node.name]
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
            if (
                (self.future_annotations or self.eager_annotations)
                and class_annotation_items
                and "__annotations__" not in class_attr_values
            ):
                ann_items: list[MoltValue] = []
                for name, val in class_annotation_items:
                    key_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                    ann_items.extend([key_val, val])
                ann_dict = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_NEW", args=ann_items, result=ann_dict))
                akey = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(kind="CONST_STR", args=["__annotations__"], result=akey)
                )
                class_def_attrs.append((akey, ann_dict))
            if (
                not self.future_annotations
                and not self.eager_annotations
                and self.class_annotation_items
                and "__annotations__" not in class_attr_values
            ):
                class_scope_names = set(class_attr_values) | set(methods)
                rewritten_items: list[tuple[str, ast.expr, int]] = []
                for name, expr, exec_id in self.class_annotation_items:
                    rewritten = self._rewrite_class_annotation_expr(
                        expr, node.name, class_scope_names
                    )
                    rewritten_items.append((name, rewritten, exec_id))
                annotate_val = self._emit_annotate_function_obj(
                    items=rewritten_items,
                    exec_map_name=self.class_annotation_exec_name,
                    stringize=False,
                    module_override=module_name,
                )
                akey = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=["__annotate__"], result=akey))
                class_def_attrs.append((akey, annotate_val))
                # Also emit eager __annotations__ dict: our runtime does not
                # implement the type.__annotations__ descriptor that CPython 3.14
                # uses to lazily evaluate __annotate__.  Eagerly storing the
                # annotations ensures cls.__annotations__ works for dataclasses
                # and any code that reads annotations directly.
                ann_items_eager: list[MoltValue] = []
                for name, expr, _exec_id in self.class_annotation_items:
                    ekey = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[name], result=ekey))
                    eval_val = self._emit_annotation_value(expr, stringize=False)
                    ann_items_eager.extend([ekey, eval_val])
                ann_dict = MoltValue(self.next_var(), type_hint="dict")
                self.emit(
                    MoltOp(kind="DICT_NEW", args=ann_items_eager, result=ann_dict)
                )
                ann_key = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(kind="CONST_STR", args=["__annotations__"], result=ann_key)
                )
                class_def_attrs.append((ann_key, ann_dict))
            for method_name, method_info in methods.items():
                mkey = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[method_name], result=mkey))
                class_def_attrs.append((mkey, method_info["attr"]))
            if class_info.get("dataclass"):
                marker_val = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=marker_val))
                dkey = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(kind="CONST_STR", args=["__molt_dataclass__"], result=dkey)
                )
                class_def_attrs.append((dkey, marker_val))
            class_def_flags = 1 if base_vals else 0
            class_def_args: list[Any] = [name_val] + list(base_vals)
            for k, v in class_def_attrs:
                class_def_args.append(k)
                class_def_args.append(v)
            layout_version = self.classes[node.name].get("layout_version", 0)
            class_def_meta = f"{len(base_vals)},{len(class_def_attrs)},{class_info['size']},{layout_version},{class_def_flags}"
            self.emit(
                MoltOp(
                    kind="CLASS_DEF",
                    args=class_def_args,
                    result=class_val,
                    metadata={"s_value": class_def_meta},
                )
            )
            self._publish_class_value(node.name, class_val)
            # ``@dataclass`` runtime application is construction-method-agnostic:
            # it operates on the finished ``class_val`` via ``setattr`` /
            # ``cls.x = ...`` and reads ``cls.__annotations__`` (published into the
            # namespace at compile time).  Apply it here for the static-outlined
            # path; the ``dynamic_build`` branch below applies the SAME helper, so
            # a dataclass whose body needs block execution (P0 #50) still gets its
            # generated dunders.
            class_val = self._emit_dataclass_application(node, class_info, class_val)
        else:
            # Dynamic path
            self._publish_class_value(node.name, class_val)
            offsets_dict_d: MoltValue | None = None
            if (
                not class_info.get("dataclass")
                and not class_info.get("dynamic")
                and class_info.get("fields")
            ):
                field_items_d: list[MoltValue] = []
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
                    field_items_d.extend([key_val, offset_val])
                offsets_dict_d = MoltValue(self.next_var(), type_hint="dict")
                self.emit(
                    MoltOp(kind="DICT_NEW", args=field_items_d, result=offsets_dict_d)
                )
            size_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[class_info["size"]], result=size_val))
            offsets_arg = offsets_dict_d
            if offsets_arg is None:
                offsets_arg = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=offsets_arg))
            self.emit(
                MoltOp(
                    kind="CLASS_MERGE_LAYOUT",
                    args=[class_val, offsets_arg, size_val],
                    result=MoltValue("none"),
                )
            )
            if (
                not self.future_annotations
                and not self.eager_annotations
                and self.class_annotation_items
                and "__annotations__" not in class_attr_values
            ):
                class_scope_names = set(class_attr_values) | set(methods)
                rewritten_items_d: list[tuple[str, ast.expr, int]] = []
                for name, expr, exec_id in self.class_annotation_items:
                    rewritten = self._rewrite_class_annotation_expr(
                        expr, node.name, class_scope_names
                    )
                    rewritten_items_d.append((name, rewritten, exec_id))
                annotate_val_d = self._emit_annotate_function_obj(
                    items=rewritten_items_d,
                    exec_map_name=self.class_annotation_exec_name,
                    stringize=False,
                    module_override=module_name,
                )
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[class_val, "__annotate__", annotate_val_d],
                        result=MoltValue("none"),
                    )
                )
            self.emit(
                MoltOp(
                    kind="CLASS_APPLY_SET_NAME",
                    args=[class_val],
                    result=MoltValue("none"),
                )
            )
            layout_version = self.classes[node.name].get("layout_version", 0)
            layout_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[layout_version], result=layout_val))
            self.emit(
                MoltOp(
                    kind="CLASS_SET_LAYOUT_VERSION",
                    args=[class_val, layout_val],
                    result=MoltValue("none"),
                )
            )
            # ``@dataclass`` runtime application on the dynamic-build path (P0
            # #50).  A dataclass whose body needs block execution (control flow /
            # ``del`` / non-Name assign target) is routed through ``dynamic_build``
            # and built via the metaclass call; the dataclass transform — which
            # installs ``__init__`` / ``__repr__`` / ``__eq__`` / ``__hash__`` /
            # frozen guards onto the finished class — must still run.  This is the
            # SAME helper the static-outlined path calls: one code path publishes
            # the dataclass transform regardless of how the class object was built.
            class_val = self._emit_dataclass_application(node, class_info, class_val)
        # Fill the ``__class__`` cell threaded into method closures with the
        # freshly built class object.  The ``dynamic_build`` (metaclass) path
        # fills it from the metaclass call's result earlier; here we cover the
        # outlined / dynamic-layout non-metaclass paths.  CPython binds the
        # ``__class__`` cell to the class produced by the class statement BEFORE
        # any class decorators run, so this fill must precede decorator
        # application below.
        if not dynamic_build and needs_classcell and classcell_val is not None:
            zero_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero_val))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[classcell_val, zero_val, class_val],
                    result=MoltValue("none"),
                )
            )
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
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(kind="CALL_BIND", args=[decorator_val, callargs], result=res)
                )
                decorated = res
            class_val = decorated
            self._publish_class_value(node.name, class_val)

        bound_class = self.globals.get(node.name)
        if (
            self.current_func_name == "molt_main"
            and not decorator_vals
            and not dynamic_build
            and bound_class is not None
            and bound_class.name == class_val.name
            and (
                not class_info.get("dataclass")
                or not class_info.get("dataclass_params", {}).get("slots", False)
            )
        ):
            class_info["class_value_name"] = class_val.name
            if self._class_constructor_fold_safe(node.name, class_info):
                class_info["constructor_fold_safe"] = True
            else:
                class_info.pop("constructor_fold_safe", None)
        else:
            class_info.pop("class_value_name", None)
            class_info.pop("constructor_fold_safe", None)

        self.class_annotation_items = prev_class_annotations
        self.class_annotation_exec_map = prev_class_exec_map
        self.class_annotation_exec_name = prev_class_exec_name
        self.class_annotation_exec_counter = prev_class_exec_counter
        self.annotation_type_params = prev_type_params
        return None

    def _method_needs_classcell_closure(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> bool:
        """True when ``node`` (a class method body being compiled) must
        receive the enclosing class's ``__class__`` cell as a closure free
        variable.

        This is the per-method companion to the class-level ``needs_classcell``
        decision: a method participates in the ``__class__`` closure iff it
        references zero-arg ``super()`` or ``__class__`` (directly or through a
        nested function/comprehension/lambda) AND the enclosing class actually
        created a ``__class__`` cell (``self._active_classcell_cell``).  When
        true, the method is compiled as a closure even at module scope so the
        cell — filled with the finished class object after the metaclass call —
        is what ``super()``/``__class__`` reads, identical to CPython.
        """
        return (
            self._active_classcell_cell is not None
            and self._function_needs_classcell(node)
        )

    def _compute_method_closure(
        self, item: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> tuple[list[str], dict[str, str], MoltValue | None, bool]:
        """Compute the closure for a class method being compiled.

        Returns ``(free_vars, free_var_hints, closure_val, has_closure)``.

        Two cases produce a closure:

        * The class body is itself nested inside a function (``current_func_name
          != "molt_main"``): the method may close over enclosing-function locals
          and, if it uses ``super()``/``__class__``, the injected ``__class__``
          cell — both captured by ``_collect_free_vars``.
        * The class body is at module scope (``molt_main``) but the method uses
          ``super()``/``__class__``: module-level names resolve as globals (not
          free vars), so the *only* closure variable is the implicit
          ``__class__`` cell.  Capturing the general free-var set here would
          wrongly demote module globals to free vars, so this case threads
          exactly ``["__class__"]``.

        Centralizing this here keeps the regular / generator / async / decorated
        method-compilation paths byte-identical and avoids re-deriving the
        ``molt_main`` vs nested decision four times.
        """
        free_vars: list[str] = []
        if self.current_func_name != "molt_main":
            free_vars = self._collect_free_vars(item)
        elif self._method_needs_classcell_closure(item):
            free_vars = ["__class__"]

        free_var_hints: dict[str, str] = {}
        closure_val: MoltValue | None = None
        has_closure = False
        if free_vars:
            self.unbound_check_names.update(free_vars)
            for name in free_vars:
                self._box_local(name)
                self.closure_locals.add(name)
            for name in free_vars:
                hint = self.boxed_local_hints.get(name)
                if hint is None:
                    value = self.locals.get(name)
                    if value is not None and value.type_hint:
                        hint = value.type_hint
                free_var_hints[name] = hint or "Any"
            closure_items = self._closure_cells_for(free_vars)
            closure_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val))
            has_closure = True
        return free_vars, free_var_hints, closure_val, has_closure

    def _method_inline_closure_ok(
        self,
        free_vars: list[str],
        item: "ast.FunctionDef | ast.AsyncFunctionDef",
    ) -> bool:
        """True when a method that is technically a *closure* is still safe to
        record as inline-eligible.

        A method using zero-arg ``super()`` closes over the enclosing class's
        implicit ``__class__`` cell, so its ``free_vars`` contains ``"__class__"``
        even though it captures no *real* enclosing locals.  That cell exists
        only to let the runtime dispatch path read the finished class object; at
        an **inline** call site the recursive static ``super()`` fold (which sets
        ``current_class`` / ``current_method_first_param`` to the inline owner)
        resolves the super-chain at compile time and never reads the cell.  When
        a ``super()`` in the inlined body cannot fold statically, the inline is
        aborted via ``_InlineSuperFoldRequired`` and the call routes through the
        cell-threaded dispatch path — so the cell is never needed at the inline
        site.

        The exception is a **bare ``__class__`` value load** (e.g.
        ``__class__.__name__``): that reads the cell directly, and the inlined
        body — spliced into a scope with no ``__class__`` cell — cannot reproduce
        it, so such a method must NOT be inline-eligible.  ``super()`` calls
        never appear as an ``ast.Name("__class__")`` (the cell binding is
        implicit), so any ``ast.Name`` with id ``"__class__"`` in the body is a
        bare value load that disqualifies the method.

        So a closure method is inline-eligible iff (a) it captures nothing beyond
        the implicit ``__class__`` cell, and (b) it contains no bare
        ``__class__`` value load.  Any genuine enclosing-local capture forces the
        CALL path, where the real closure tuple is threaded.
        """
        if not free_vars:
            return True
        real = [name for name in free_vars if name != "__class__"]
        if real:
            return False
        # ``free_vars == ["__class__"]``: eligible only if the body uses the
        # cell exclusively through ``super()`` (no bare ``__class__`` value).
        for child in ast.walk(item):
            if isinstance(child, ast.Name) and child.id == "__class__":
                return False
        return True

    def _inline_body_external_names(
        self, expr: "ast.expr", params: list[str]
    ) -> "frozenset[str]":
        """Collect the bare ``Name`` *loads* in an inline-body expression that
        are neither substituted parameters nor builtins.

        At inline time the body is spliced into the caller's scope and the
        parameters are substituted (``self.locals = {param: arg_value}``).  A
        bare ``Name`` that is NOT a parameter falls through ``visit_Name`` to
        ``_emit_global_get`` and resolves against the **caller's** module
        globals — which is only correct when the call site is compiled in the
        method's *defining* module.  A reference to the defining module's own
        global (a module-level constant, a sibling function, an intrinsic
        binding such as ``_MOLT_ARRAY_TOLIST``) therefore silently mis-resolves
        across a module boundary, yielding a ``NameError`` at runtime.

        Builtins (``len``, ``super``, exception/type names, …) are excluded
        because ``visit_Name`` materialises them statically and identically in
        every scope, so they remain sound under cross-module splicing.

        The returned set drives ``_try_inline_method_call``'s cross-module
        refusal (the fail-closed soundness gate).
        """
        param_set = set(params)
        external: set[str] = set()
        for node in ast.walk(expr):
            if not isinstance(node, ast.Name):
                continue
            if not isinstance(node.ctx, ast.Load):
                continue
            name = node.id
            if name in param_set:
                continue
            if self._name_resolves_to_builtin(name):
                continue
            external.add(name)
        return frozenset(external)

    def _extract_inline_return(
        self, item: "ast.FunctionDef", params: list[str]
    ) -> "ast.expr | None":
        """Return the body's `return <expr>` AST iff the method is
        trivially inlinable: body is a single Return statement (an
        optional leading docstring is allowed), the returned expression
        references only parameters, attributes-on-parameters,
        constants, builtins, and pure operations (BinOp, UnaryOp,
        Compare, BoolOp, IfExp, Tuple/List of inlinable elements), plus
        nested Calls thereof.

        The returned expression MAY reference globals of the *defining*
        module (e.g. ``_MOLT_ARRAY_TOLIST(self._handle)``); such bodies
        are still inline-eligible but ``_try_inline_method_call`` refuses
        to splice them across a module boundary (where the global would
        mis-resolve).  See ``_inline_body_external_names``.
        """
        body = item.body
        # Skip docstring if present.
        if (
            body
            and isinstance(body[0], ast.Expr)
            and isinstance(body[0].value, ast.Constant)
        ):
            body = body[1:]
        if (
            len(body) != 1
            or not isinstance(body[0], ast.Return)
            or body[0].value is None
        ):
            return None

        def _safe(node: "ast.AST") -> bool:
            if isinstance(node, ast.Constant):
                return True
            if isinstance(node, ast.Name):
                # Builtin names like `super` are allowed even though
                # they aren't params — visit_Call's Phase 4a / general
                # path handles them correctly with the inline scope's
                # current_class set.  Plain identifier refs that
                # AREN'T params would fail at visit time (KeyError on
                # the substituted self.locals), which the caller
                # catches and bails on.
                return True
            if isinstance(node, ast.Attribute):
                return _safe(node.value)
            if isinstance(node, ast.BinOp):
                return _safe(node.left) and _safe(node.right)
            if isinstance(node, ast.UnaryOp):
                return _safe(node.operand)
            if isinstance(node, ast.Compare):
                return _safe(node.left) and all(_safe(c) for c in node.comparators)
            if isinstance(node, ast.BoolOp):
                return all(_safe(v) for v in node.values)
            if isinstance(node, ast.IfExp):
                return _safe(node.test) and _safe(node.body) and _safe(node.orelse)
            if (
                isinstance(node, (ast.Tuple, ast.List))
                and not getattr(node, "ctx", None).__class__.__name__ == "Store"
            ):
                return all(_safe(e) for e in node.elts)
            # Calls are allowed: when visited at inline time, Phase 4a's
            # super() fold or Phase 1's user-method fold will recursively
            # try inlining or emit a direct CALL.  Either way the body
            # composes cleanly with the substitution model — the Names
            # in arg positions get resolved against the inline locals.
            if isinstance(node, ast.Call):
                # Reject calls with kwargs / starred args — defensive,
                # the substitution model handles only positional.
                if node.keywords:
                    return False
                if any(isinstance(a, ast.Starred) for a in node.args):
                    return False
                return _safe(node.func) and all(_safe(a) for a in node.args)
            return False

        return body[0].value if _safe(body[0].value) else None

    def _extract_inline_init_assigns(
        self, item: "ast.FunctionDef", params: list[str]
    ) -> "list[tuple[str, ast.expr]] | None":
        """Detect `__init__`-style trivially-inlinable bodies.

        Accepts a body that is a sequence of `self.attr = <pure expr>`
        assignments (an optional leading docstring is allowed), where:
          - the assignment target is `Attribute(Name(<first param>),
            <attr>)` — i.e. `self.attr` for whatever `self` is named.
          - the value expression is `_safe` per `_extract_inline_return`'s
            criteria (constants, params, attributes-on-params, pure
            BinOp/UnaryOp/Compare/etc., Calls).
          - no other statement kinds (no `if`, no `for`, no `try`, no
            extra `Assign` to non-self targets, no `Return` other than
            implicit None).

        Returns the list of `(attr_name, expr_AST)` pairs in order, or
        ``None`` if the body doesn't match the pattern.

        At inline time the caller substitutes params → call args and
        emits a STORE_ATTR per pair on the freshly-allocated instance,
        eliminating the __init__ CALL frame setup that dominates
        bench_struct's per-iter cost.
        """
        if not params:
            return None
        self_name = params[0]
        body = item.body
        # Skip docstring if present.
        if (
            body
            and isinstance(body[0], ast.Expr)
            and isinstance(body[0].value, ast.Constant)
        ):
            body = body[1:]
        if not body:
            # Empty body (only docstring) — equivalent to a no-op
            # __init__.  Inline as zero stores; still a perf win since
            # we skip the CALL.
            return []

        def _safe(node: "ast.AST") -> bool:
            if isinstance(node, ast.Constant):
                return True
            if isinstance(node, ast.Name):
                return True
            if isinstance(node, ast.Attribute):
                return _safe(node.value)
            if isinstance(node, ast.BinOp):
                return _safe(node.left) and _safe(node.right)
            if isinstance(node, ast.UnaryOp):
                return _safe(node.operand)
            if isinstance(node, ast.Compare):
                return _safe(node.left) and all(_safe(c) for c in node.comparators)
            if isinstance(node, ast.BoolOp):
                return all(_safe(v) for v in node.values)
            if isinstance(node, ast.IfExp):
                return _safe(node.test) and _safe(node.body) and _safe(node.orelse)
            if (
                isinstance(node, (ast.Tuple, ast.List))
                and not getattr(node, "ctx", None).__class__.__name__ == "Store"
            ):
                return all(_safe(e) for e in node.elts)
            if isinstance(node, ast.Call):
                if node.keywords:
                    return False
                if any(isinstance(a, ast.Starred) for a in node.args):
                    return False
                return _safe(node.func) and all(_safe(a) for a in node.args)
            return False

        assigns: list[tuple[str, ast.expr]] = []
        for stmt in body:
            # Allow trailing `return` / `return None` — Python __init__
            # implicitly returns None, but explicit `return None` is
            # also legal.  Anything else fails the pattern.
            if isinstance(stmt, ast.Return):
                if stmt.value is None:
                    continue
                if isinstance(stmt.value, ast.Constant) and stmt.value.value is None:
                    continue
                return None
            if not isinstance(stmt, ast.Assign):
                return None
            if len(stmt.targets) != 1:
                return None
            target = stmt.targets[0]
            if not isinstance(target, ast.Attribute):
                return None
            if not isinstance(target.value, ast.Name):
                return None
            if target.value.id != self_name:
                return None
            if not _safe(stmt.value):
                return None
            assigns.append((target.attr, stmt.value))
        return assigns
