"""Compiled projections of the dataclass transform's decoration-time schema.

These are candidate bodies, never authority to allocate an instance or install
an initializer. The transform compares the actual field/signature schema before
using a candidate. Defaults and factories are captured at that transformation.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from molt.compiler_analysis.python_binding_facts import PythonBindingIndex
from molt.compiler_analysis.python_identity_catalog_generated import PythonIdentity


@dataclass(frozen=True)
class DataclassInitField:
    name: str
    init: bool
    kw_only: bool
    initvar: bool
    # 0: required, 1: ordinary default, 2: factory.
    default: int

    def schema(self) -> tuple[str, bool, bool, bool, int]:
        return self.name, self.init, self.kw_only, self.initvar, self.default

    @classmethod
    def from_schema(cls, schema: object) -> DataclassInitField:
        """The inverse of ``schema``; a malformed schema raises ValueError."""
        if (
            not isinstance(schema, (list, tuple))
            or len(schema) != 5
            or not isinstance(schema[0], str)
            or not schema[0].isidentifier()
            or not all(type(flag) is bool for flag in schema[1:4])
            or type(schema[4]) is not int
            or schema[4] not in (0, 1, 2)
        ):
            raise ValueError(
                f"malformed dataclass initializer field schema: {schema!r}"
            )
        return cls(schema[0], schema[1], schema[2], schema[3], schema[4])


def initializer_template(
    fields: tuple[DataclassInitField, ...],
    *,
    frozen: bool,
    slots: bool,
    post_init: bool,
    location: ast.ClassDef,
) -> ast.FunctionDef:
    """Build an ordinary factory and initializer through the Python AST pipeline."""

    def name(value: str, ctx: ast.expr_context | None = None) -> ast.Name:
        return ast.Name(id=value, ctx=ctx or ast.Load())

    def call(function: ast.expr, args: list[ast.expr]) -> ast.Call:
        return ast.Call(func=function, args=args, keywords=[])

    receiver = "__dataclass_self__" if any(f.name == "self" for f in fields) else "self"
    captures: set[str] = set()
    body: list[ast.stmt] = []
    initvars: list[ast.expr] = []
    for field in fields:
        if field.initvar:
            initvars.append(name(field.name))
            continue
        value: ast.expr
        if field.default == 2:
            factory = f"__dataclass_dflt_{field.name}__"
            captures.add(factory)
            value = call(name(factory), [])
            if field.init:
                captures.add("__dataclass_HAS_DEFAULT_FACTORY__")
                value = ast.IfExp(
                    test=ast.Compare(
                        left=name(field.name),
                        ops=[ast.Is()],
                        comparators=[name("__dataclass_HAS_DEFAULT_FACTORY__")],
                    ),
                    body=value,
                    orelse=name(field.name),
                )
        elif field.init:
            value = name(field.name)
        elif slots and field.default == 1:
            captures.add(f"__dataclass_dflt_{field.name}__")
            value = name(f"__dataclass_dflt_{field.name}__")
        else:
            continue
        if frozen:
            captures.add("__dataclass_builtins_object__")
            body.append(
                ast.Expr(
                    value=call(
                        ast.Attribute(
                            value=name("__dataclass_builtins_object__"),
                            attr="__setattr__",
                            ctx=ast.Load(),
                        ),
                        [name(receiver), ast.Constant(field.name), value],
                    )
                )
            )
        else:
            body.append(
                ast.Assign(
                    targets=[
                        ast.Attribute(
                            value=name(receiver), attr=field.name, ctx=ast.Store()
                        )
                    ],
                    value=value,
                )
            )
    if post_init:
        body.append(
            ast.Expr(
                value=call(
                    ast.Attribute(
                        value=name(receiver), attr="__post_init__", ctx=ast.Load()
                    ),
                    initvars,
                )
            )
        )
    if not body:
        body.append(ast.Pass())
    parameters = [f.name for f in fields if f.init and not f.kw_only]
    keyword_only = [f.name for f in fields if f.init and f.kw_only]
    initializer = ast.FunctionDef(
        name="__init__",
        args=ast.arguments(
            posonlyargs=[],
            args=[ast.arg(arg=n) for n in [receiver, *parameters]],
            vararg=None,
            kwonlyargs=[ast.arg(arg=n) for n in keyword_only],
            kw_defaults=[None] * len(keyword_only),
            kwarg=None,
            defaults=[],
        ),
        body=body,
        decorator_list=[],
        returns=None,
        type_params=[],
    )
    captures.difference_update([receiver, *parameters, *keyword_only])
    factory_body: list[ast.stmt] = [
        ast.Assign(
            targets=[name(capture, ast.Store())],
            value=ast.Subscript(
                value=name("captures"), slice=ast.Constant(i), ctx=ast.Load()
            ),
        )
        for i, capture in enumerate(sorted(captures))
    ]
    factory_body.extend([initializer, ast.Return(value=name("__init__"))])
    factory = ast.FunctionDef(
        name="__molt_dataclass_init_template__",
        args=ast.arguments(
            posonlyargs=[],
            args=[ast.arg(arg="captures")],
            vararg=None,
            kwonlyargs=[],
            kw_defaults=[],
            kwarg=None,
            defaults=[],
        ),
        body=factory_body,
        decorator_list=[],
        returns=None,
        type_params=[],
    )
    return ast.fix_missing_locations(ast.copy_location(factory, location))


def literal_dataclass_options(decorator: ast.expr) -> dict[str, bool] | None:
    """Project literal options; this never classifies the decorator's value."""
    options = dict(
        init=True,
        repr=True,
        eq=True,
        order=False,
        unsafe_hash=False,
        frozen=False,
        match_args=True,
        kw_only=False,
        slots=False,
        weakref_slot=False,
    )
    if isinstance(decorator, ast.Call):
        if decorator.args:
            return None
        for keyword in decorator.keywords:
            if keyword.arg not in options or not (
                isinstance(keyword.value, ast.Constant)
                and type(keyword.value.value) is bool
            ):
                return None
            options[keyword.arg] = keyword.value.value
    return options


def project_initializer_fields(
    node: ast.ClassDef,
    inherited: tuple[tuple[DataclassInitField, ...], ...],
    kw_only: bool,
    index: PythonBindingIndex | None,
) -> tuple[DataclassInitField, ...] | None:
    """A proposal certified by the transform, never an annotation/type fact.

    An uncertain decorator or annotation does not authorize dataclass semantics.
    The real transform compares every field/flag/default role before installing
    this ordinary function. Unknown annotation values project regular fields;
    markers or callbacks that disagree use their actual decoration-time plan.
    """
    if any(
        isinstance(item, (ast.If, ast.For, ast.While, ast.Try, ast.With, ast.Match))
        for item in node.body
    ):
        return None
    fields = {field.name: field for group in inherited for field in group}
    for item in node.body:
        if not isinstance(item, ast.AnnAssign) or not isinstance(item.target, ast.Name):
            continue
        name = item.target.id
        annotation = item.annotation
        target = (
            annotation.value if isinstance(annotation, ast.Subscript) else annotation
        )
        kind = "field"
        if index is not None:
            for identity, marker in (
                (PythonIdentity.DATACLASSES_KW_ONLY, "kw_only"),
                (PythonIdentity.DATACLASSES_INITVAR, "initvar"),
                (PythonIdentity.TYPING_CLASSVAR, "classvar"),
            ):
                fact = index.expression_fact(target)
                if fact is not None and fact.identities & int(identity):
                    kind = marker
                    break
        if kind == "kw_only":
            kw_only = True
            continue
        if kind == "classvar":
            fields.pop(name, None)
            continue
        field_init, field_kw_only = True, kw_only
        default = int(item.value is not None)
        value = item.value
        if isinstance(value, ast.Call):
            fact = index.expression_fact(value.func) if index is not None else None
            possible_field = fact is not None and bool(
                fact.identities & int(PythonIdentity.DATACLASSES_FIELD)
            )
            if possible_field:
                if value.args:
                    return None
                default = 0
                for keyword in value.keywords:
                    if keyword.arg in {"init", "kw_only"}:
                        if (
                            not isinstance(keyword.value, ast.Constant)
                            or type(keyword.value.value) is not bool
                        ):
                            return None
                        if keyword.arg == "init":
                            field_init = keyword.value.value
                        else:
                            field_kw_only = keyword.value.value
                    elif keyword.arg == "default":
                        default = 1
                    elif keyword.arg == "default_factory":
                        default = 2
                    elif keyword.arg is None:
                        return None
        fields[name] = DataclassInitField(
            name, field_init, field_kw_only, kind == "initvar", default
        )
    return tuple(fields.values())
