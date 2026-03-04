from __future__ import annotations

import json
import re
from typing import Any

from .errors import TemplateParseError, TemplateRenderError


_EXPR_RE = re.compile(r"\{\{\s*(.*?)\s*\}\}")
_TAG_RE = re.compile(r"\{%.*?%\}", re.DOTALL)


def render_prompt(template: str, issue: dict[str, Any], attempt: int | None) -> str:
    if _TAG_RE.search(template):
        raise TemplateParseError("template_parse_error unsupported tag syntax")

    context: dict[str, Any] = {
        "issue": issue,
        "attempt": attempt,
    }

    def replace(match: re.Match[str]) -> str:
        expression = match.group(1).strip()
        if not expression:
            raise TemplateParseError("template_parse_error empty expression")
        value = _eval_expression(expression, context)
        if value is None:
            return ""
        if isinstance(value, (dict, list, tuple)):
            return json.dumps(value, ensure_ascii=True)
        return str(value)

    try:
        rendered = _EXPR_RE.sub(replace, template)
    except TemplateParseError:
        raise
    except TemplateRenderError:
        raise
    except Exception as exc:  # pragma: no cover - defensive
        raise TemplateRenderError(f"template_render_error {exc}") from exc
    return rendered


def _eval_expression(expression: str, context: dict[str, Any]) -> Any:
    parts = [part.strip() for part in expression.split("|")]
    value = _resolve_variable(parts[0], context)
    for filter_expr in parts[1:]:
        value = _apply_filter(filter_expr, value)
    return value


def _resolve_variable(path: str, context: dict[str, Any]) -> Any:
    if not path:
        raise TemplateParseError("template_parse_error empty variable path")

    segments = [segment.strip() for segment in path.split(".")]
    current: Any = context
    for segment in segments:
        if not segment:
            raise TemplateParseError(f"template_parse_error invalid path={path}")
        if isinstance(current, dict):
            if segment not in current:
                raise TemplateRenderError(
                    f"template_render_error unknown variable={path}"
                )
            current = current[segment]
            continue
        raise TemplateRenderError(f"template_render_error unknown variable={path}")
    return current


def _apply_filter(filter_expr: str, value: Any) -> Any:
    if not filter_expr:
        raise TemplateParseError("template_parse_error empty filter")

    name, arg = _split_filter(filter_expr)
    if name == "upcase":
        return "" if value is None else str(value).upper()
    if name == "downcase":
        return "" if value is None else str(value).lower()
    if name == "strip":
        return "" if value is None else str(value).strip()
    if name == "json":
        return json.dumps(value, ensure_ascii=True)
    if name == "default":
        if arg is None:
            raise TemplateParseError(
                "template_parse_error default filter needs argument"
            )
        if value in (None, "", [], (), {}):
            return arg
        return value
    raise TemplateRenderError(f"template_render_error unknown filter={name}")


def _split_filter(filter_expr: str) -> tuple[str, str | None]:
    if ":" not in filter_expr:
        return filter_expr.strip(), None
    raw_name, raw_arg = filter_expr.split(":", 1)
    name = raw_name.strip()
    arg = raw_arg.strip()
    if arg.startswith('"') and arg.endswith('"') and len(arg) >= 2:
        arg = arg[1:-1]
    if arg.startswith("'") and arg.endswith("'") and len(arg) >= 2:
        arg = arg[1:-1]
    return name, arg
