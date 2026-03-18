"""Translation Validation hooks for per-pass IR snapshot emission.

When the environment variable ``MOLT_TV_EMIT=1`` is set, these hooks
serialise the before/after IR for each mid-end pass to a temporary
directory.  The snapshots can then be validated offline by
``tools/translation_validator.py``.

Usage from inside the mid-end (without modifying ``__init__.py``)::

    from molt.frontend.tv_hooks import tv_emit_before, tv_emit_after, tv_active

    if tv_active():
        tv_emit_before(function_name, pass_name, ops)
    ...  # run the pass
    if tv_active():
        tv_emit_after(function_name, pass_name, ops)

The dump directory defaults to ``$MOLT_TV_DIR`` if set, otherwise a
fresh ``tempfile.mkdtemp`` under ``$TMPDIR`` (or ``/tmp``).  The path
is printed to stderr on first use so external tooling can discover it.

File naming convention::

    {function}_{pass}_before.json
    {function}_{pass}_after.json

Each JSON file contains::

    {
      "function": "<name>",
      "pass": "<pass_name>",
      "phase": "before" | "after",
      "op_count": <int>,
      "ops": [
        {
          "kind": "...",
          "args": [...],
          "result": {"name": "...", "type_hint": "..."},
          "metadata": {...} | null
        },
        ...
      ]
    }
"""

from __future__ import annotations

import json
import os
import re
import sys
import tempfile
from pathlib import Path
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    pass

# ---------------------------------------------------------------------------
# Module-level state
# ---------------------------------------------------------------------------

_tv_enabled: bool | None = None
_dump_dir: Path | None = None


def tv_active() -> bool:
    """Return True if TV emission is enabled (``MOLT_TV_EMIT=1``)."""
    global _tv_enabled
    if _tv_enabled is None:
        _tv_enabled = os.environ.get("MOLT_TV_EMIT", "0") == "1"
    return _tv_enabled


def tv_dump_dir() -> Path:
    """Return (and lazily create) the dump directory."""
    global _dump_dir
    if _dump_dir is None:
        explicit = os.environ.get("MOLT_TV_DIR")
        if explicit:
            _dump_dir = Path(explicit)
            _dump_dir.mkdir(parents=True, exist_ok=True)
        else:
            _dump_dir = Path(
                tempfile.mkdtemp(
                    prefix="molt_tv_",
                    dir=os.environ.get("TMPDIR", "/tmp"),
                )
            )
        print(
            f"[molt-tv] dump directory: {_dump_dir}",
            file=sys.stderr,
        )
    return _dump_dir


def reset() -> None:
    """Reset module state (for testing)."""
    global _tv_enabled, _dump_dir
    _tv_enabled = None
    _dump_dir = None


# ---------------------------------------------------------------------------
# Serialisation
# ---------------------------------------------------------------------------

# Regex to sanitise function names for use in filenames.
_SAFE_NAME_RE = re.compile(r"[^a-zA-Z0-9_]")


def _safe_name(name: str) -> str:
    """Convert a function name to a safe filesystem component."""
    safe = _SAFE_NAME_RE.sub("_", name)
    # Collapse runs of underscores and trim
    safe = re.sub(r"_+", "_", safe).strip("_")
    return safe[:120] or "anon"


def _serialise_op(op: Any) -> dict[str, Any]:
    """Serialise a MoltOp (or already-dict op) to a JSON-safe dict.

    Handles both live ``MoltOp`` dataclass instances and pre-serialised
    dicts (for idempotency when re-processing snapshots).
    """
    if isinstance(op, dict):
        return op

    # Live MoltOp dataclass
    result_dict: dict[str, str]
    result = getattr(op, "result", None)
    if result is not None and hasattr(result, "name"):
        result_dict = {
            "name": result.name,
            "type_hint": getattr(result, "type_hint", "Unknown"),
        }
    else:
        result_dict = {"name": "none", "type_hint": "Unknown"}

    return {
        "kind": getattr(op, "kind", "UNKNOWN"),
        "args": _serialise_args(getattr(op, "args", [])),
        "result": result_dict,
        "metadata": _serialise_metadata(getattr(op, "metadata", None)),
    }


def _serialise_args(args: Any) -> Any:
    """Recursively serialise an args list to JSON-safe values."""
    if args is None:
        return []
    if isinstance(args, list):
        return [_serialise_args(a) for a in args]
    if isinstance(args, tuple):
        return [_serialise_args(a) for a in args]
    if isinstance(args, dict):
        return {str(k): _serialise_args(v) for k, v in args.items()}
    # MoltValue
    if hasattr(args, "name") and hasattr(args, "type_hint"):
        return {"name": args.name, "type_hint": args.type_hint}
    # Primitives
    if isinstance(args, (int, float, str, bool, type(None))):
        return args
    # Fallback: repr
    try:
        return repr(args)
    except Exception:
        return "<unserializable>"


def _serialise_metadata(metadata: Any) -> Any:
    """Serialise op metadata to a JSON-safe value."""
    if metadata is None:
        return None
    if isinstance(metadata, dict):
        safe: dict[str, Any] = {}
        for k, v in metadata.items():
            try:
                json.dumps(v)
                safe[str(k)] = v
            except (TypeError, ValueError):
                safe[str(k)] = repr(v)
        return safe
    return repr(metadata)


def _emit(
    function_name: str,
    pass_name: str,
    phase: str,
    ops: Any,
) -> Path:
    """Write a single snapshot file and return its path."""
    dump = tv_dump_dir()
    safe_func = _safe_name(function_name)
    safe_pass = _safe_name(pass_name)

    ops_list: list[Any]
    if isinstance(ops, list):
        ops_list = ops
    else:
        ops_list = list(ops)

    payload: dict[str, Any] = {
        "function": function_name,
        "pass": pass_name,
        "phase": phase,
        "op_count": len(ops_list),
        "ops": [_serialise_op(op) for op in ops_list],
    }

    filename = f"{safe_func}_{safe_pass}_{phase}.json"
    path = dump / filename
    path.write_text(json.dumps(payload, indent=2), encoding="utf-8")
    return path


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def tv_emit_before(
    function_name: str,
    pass_name: str,
    ops: Any,
) -> Path | None:
    """Emit a ``*_before.json`` snapshot.  No-op if TV is not active."""
    if not tv_active():
        return None
    return _emit(function_name, pass_name, "before", ops)


def tv_emit_after(
    function_name: str,
    pass_name: str,
    ops: Any,
) -> Path | None:
    """Emit a ``*_after.json`` snapshot.  No-op if TV is not active."""
    if not tv_active():
        return None
    return _emit(function_name, pass_name, "after", ops)
