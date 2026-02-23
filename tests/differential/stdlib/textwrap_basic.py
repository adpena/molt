"""Purpose: differential coverage for textwrap wrapper + option contracts."""

from __future__ import annotations

import textwrap


def _supports_textwrapper_option(text: str, **kwargs: object) -> bool:
    try:
        textwrap.TextWrapper(width=8, **kwargs).wrap(text)
    except TypeError:
        return False
    except RuntimeError as exc:
        if "require runtime intrinsic support" in str(exc):
            return False
        raise
    return True


def _assert_textwrapper_option_rejected(text: str, option_name: str, **kwargs: object) -> None:
    try:
        textwrap.TextWrapper(width=8, **kwargs).wrap(text)
    except TypeError:
        return
    except RuntimeError as exc:
        message = str(exc)
        assert "require runtime intrinsic support" in message, message
        assert option_name in message, message
        return
    raise AssertionError(f"expected TextWrapper to reject options: {sorted(kwargs)}")


def _supports_indent_predicate() -> bool:
    try:
        textwrap.indent("x", "> ", lambda _: True)
    except TypeError:
        return False
    except RuntimeError:
        return False
    return True


wrapper = textwrap.TextWrapper(width=5)
wrapped = wrapper.wrap("hello world from molt")
filled = wrapper.fill("hello world from molt")
print("width_wrap", wrapped)
print("width_fill", filled.replace("\n", "|"))
assert wrapped == ["hello", "world", "from", "molt"]
assert filled == "hello\nworld\nfrom\nmolt"
assert "\n".join(wrapped) == filled

if _supports_indent_predicate():
    indented_default = textwrap.indent("line1\n\nline2", "> ")
    indented_all = textwrap.indent("line1\n\nline2", "> ", lambda _: True)
    assert indented_default == "> line1\n\n> line2"
    assert indented_all == "> line1\n> \n> line2"
else:
    indented_default = textwrap.indent("line1\n\nline2", "> ")
    assert indented_default == "> line1\n> \n> line2"
    try:
        textwrap.indent("line1\n\nline2", "> ", lambda _: True)
    except (TypeError, RuntimeError):
        pass
    else:
        raise AssertionError("indent(predicate=...) should fail when predicate support is absent")
print("indent_contract", "ok")

if _supports_textwrapper_option("a   b   c", drop_whitespace=False):
    drop_true = textwrap.TextWrapper(width=6, drop_whitespace=True).wrap("a   b   c")
    drop_false = textwrap.TextWrapper(width=6, drop_whitespace=False).wrap("a   b   c")
    assert drop_true == ["a   b", "c"]
    assert drop_false == ["a   b", "   c"]
else:
    _assert_textwrapper_option_rejected(
        "a   b   c",
        "drop_whitespace",
        drop_whitespace=False,
    )
print("drop_whitespace_contract", "ok")

if _supports_textwrapper_option("a\tb", expand_tabs=False, replace_whitespace=False):
    expand_true = textwrap.TextWrapper(
        width=4,
        expand_tabs=True,
        replace_whitespace=False,
    ).wrap("a\tb")
    expand_false = textwrap.TextWrapper(
        width=4,
        expand_tabs=False,
        replace_whitespace=False,
    ).wrap("a\tb")
    assert expand_true == ["a", "b"]
    assert expand_false == ["a\tb"]
else:
    _assert_textwrapper_option_rejected(
        "a\tb",
        "expand_tabs",
        expand_tabs=False,
        replace_whitespace=False,
    )
print("expand_tabs_contract", "ok")

if _supports_textwrapper_option(
    "alpha-beta-gamma",
    break_on_hyphens=False,
    break_long_words=False,
):
    break_true = textwrap.TextWrapper(
        width=8,
        break_on_hyphens=True,
        break_long_words=False,
    ).wrap("alpha-beta-gamma")
    break_false = textwrap.TextWrapper(
        width=8,
        break_on_hyphens=False,
        break_long_words=False,
    ).wrap("alpha-beta-gamma")
    assert break_true == ["alpha-", "beta-", "gamma"]
    assert break_false == ["alpha-beta-gamma"]
else:
    _assert_textwrapper_option_rejected(
        "alpha-beta-gamma",
        "break_on_hyphens",
        break_on_hyphens=False,
        break_long_words=False,
    )
print("break_on_hyphens_contract", "ok")
