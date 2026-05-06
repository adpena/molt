from __future__ import annotations

from molt.frontend import compile_to_tir


def _function_names(source: str) -> list[str]:
    ir = compile_to_tir(source)
    return [func["name"] for func in ir["functions"]]


def test_eager_function_annotations_do_not_emit_dead_annotate_function() -> None:
    source = """
def f(value: int) -> str:
    return "ok"
"""

    names = _function_names(source)

    assert not any("__annotate__" in name for name in names)


def test_future_function_annotations_do_not_emit_dead_annotate_function() -> None:
    source = """
from __future__ import annotations

def f(value: int) -> str:
    return "ok"
"""

    names = _function_names(source)

    assert not any("__annotate__" in name for name in names)


def test_eager_function_annotations_still_materialize_annotations_dict() -> None:
    source = """
from __future__ import annotations

def f(value: int) -> str:
    return "ok"
"""
    ir = compile_to_tir(source)
    main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )

    assert any(
        op.get("kind") == "set_attr_generic_obj"
        and op.get("s_value") == "__annotations__"
        for op in main_ops
    )
