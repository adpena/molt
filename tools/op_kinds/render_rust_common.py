from __future__ import annotations

def _rs_bool(value: bool) -> str:
    return "true" if value else "false"

def _rs_string(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


# ---------------------------------------------------------------------------
# Table loading + validation
# ---------------------------------------------------------------------------

def _render_matches_arm(spellings: list[str]) -> str:
    """Render the body of a `matches!(kind, ...)` as one `|`-joined OR-pattern,
    one spelling per line, in the order given. Empty set renders a never-match
    arm so the function is still well-formed."""
    if not spellings:
        # An empty exact set means "only the prefix/None paths apply". Render a
        # single impossible literal pattern (a NUL-prefixed kind never occurs as
        # a wire spelling) so the `matches!` stays well-formed and always false.
        return '        "\\0__never__"\n'
    lines = []
    for i, s in enumerate(spellings):
        sep = "" if i == len(spellings) - 1 else " |"
        lines.append(f'        "{s}"{sep}\n')
    return "".join(lines)

def _render_opcode_bool_arms(opcodes: list[dict], truthy: list[str]) -> str:
    """Render exhaustive `OpCode::X => bool` arms in table order."""
    truthy_set = set(truthy)
    lines = []
    for row in opcodes:
        name = row["name"]
        lines.append(f"        OpCode::{name} => {_rs_bool(name in truthy_set)},\n")
    return "".join(lines)

def _opcode_const_suffix(opcode: str) -> str:
    words: list[str] = []
    current = ""
    for ch in opcode:
        if ch.isupper() and current:
            words.append(current)
            current = ch
        else:
            current += ch
    if current:
        words.append(current)
    return "_".join(w.upper() for w in words)
