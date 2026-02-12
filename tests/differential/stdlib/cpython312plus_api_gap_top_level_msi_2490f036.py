"""Purpose: CPython 3.12+ stdlib API-gap semantic probe for `_msi`."""

import contextlib
import hashlib
import importlib
import importlib.util
import io
import json

MODULE_NAME = "_msi"
SPEC_ONLY_MODULES = ['antigravity']
SPEC_ONLY_PREFIXES = ('tkinter', 'turtle', 'turtledemo')


def _digest_text(text: str) -> str:
    if not text:
        return ""
    return hashlib.sha256(text.encode("utf-8")).hexdigest()[:20]


def _spec_probe(module_name: str) -> dict[str, object]:
    spec = importlib.util.find_spec(module_name)
    if spec is None:
        return {
            "module": module_name,
            "status": "spec_missing",
        }
    locations = spec.submodule_search_locations
    return {
        "module": module_name,
        "status": "spec_only",
        "loader": type(spec.loader).__name__ if spec.loader is not None else None,
        "origin": spec.origin,
        "is_package": bool(locations),
    }


def _module_api_digest(module: object) -> tuple[int, str, list[str]]:
    values = getattr(module, "__dict__", {})
    rows: list[tuple[str, str, bool]] = []
    for name, value in sorted(values.items()):
        if name.startswith("_"):
            continue
        rows.append((name, type(value).__name__, bool(callable(value))))
    payload = json.dumps(rows, separators=(",", ":"), ensure_ascii=True)
    digest = hashlib.sha256(payload.encode("utf-8")).hexdigest()[:20]
    head = [name for name, _, _ in rows[:10]]
    return len(rows), digest, head


def _is_spec_only(module_name: str) -> bool:
    if module_name in SPEC_ONLY_MODULES:
        return True
    for prefix in SPEC_ONLY_PREFIXES:
        if module_name == prefix or module_name.startswith(prefix + "."):
            return True
    return False


def _probe(module_name: str) -> dict[str, object]:
    if _is_spec_only(module_name):
        return _spec_probe(module_name)

    cap_stdout = io.StringIO()
    cap_stderr = io.StringIO()
    try:
        with contextlib.redirect_stdout(cap_stdout), contextlib.redirect_stderr(cap_stderr):
            module = importlib.import_module(module_name)
    except BaseException as exc:  # noqa: BLE001
        return {
            "module": module_name,
            "status": "import_error",
            "error_type": type(exc).__name__,
            "error_text": str(exc),
            "stdout_digest": _digest_text(cap_stdout.getvalue()),
            "stderr_digest": _digest_text(cap_stderr.getvalue()),
        }

    public_count, api_digest, public_head = _module_api_digest(module)
    return {
        "module": module_name,
        "status": "ok",
        "public_count": public_count,
        "api_digest": api_digest,
        "public_head": public_head,
        "stdout_digest": _digest_text(cap_stdout.getvalue()),
        "stderr_digest": _digest_text(cap_stderr.getvalue()),
    }


print(json.dumps(_probe(MODULE_NAME), sort_keys=True, ensure_ascii=True))
