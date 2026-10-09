from __future__ import annotations

import argparse


def _completion_script(shell: str, *, parser: argparse.ArgumentParser) -> str:
    """Render the selected parser once; completion never rebuilds CLI authority."""
    import shlex

    if shell not in {"bash", "zsh", "fish"}:
        raise ValueError(f"Unsupported shell: {shell}")

    # IDs are local to this emitted script, not a second command inventory.
    nodes: list[argparse.ArgumentParser] = []
    children: dict[int, list[tuple[str, int]]] = {}

    def visit(current: argparse.ArgumentParser) -> int:
        index = len(nodes)
        nodes.append(current)
        children[index] = []
        for action in current._actions:
            if not isinstance(action, argparse._SubParsersAction):
                continue
            hidden = {
                id(action.choices[item.dest])
                for item in action._get_subactions()
                if item.help == argparse.SUPPRESS
            }
            for name, child in action.choices.items():
                if id(child) not in hidden:
                    children[index].append((name, visit(child)))
        return index

    visit(parser)
    transitions: list[tuple[list[str], str, str]] = []
    menus: dict[int, list[str]] = {}
    values: dict[str, list[str]] = {}
    positional_choices: list[tuple[int, int, list[str]]] = []
    remainders: dict[int, int] = {}
    for index, current in enumerate(nodes):
        menus[index] = [name for name, _ in children[index]]
        for name, child in children[index]:
            transitions.append(([f"{index}:{name}"], "child", str(child)))
        position = 0
        for action in current._actions:
            if isinstance(action, argparse._SubParsersAction):
                continue
            if not action.option_strings:
                if action.nargs == argparse.REMAINDER:
                    remainders[index] = position
                elif action.choices is not None and action.help != argparse.SUPPRESS:
                    positional_choices.append(
                        (index, position, [str(value) for value in action.choices])
                    )
                position += 1
                continue
            # The current public parser uses flags and single-value options.
            # Refuse an unrepresented future grammar instead of emitting lies.
            if action.nargs not in (None, 0, 1):
                raise ValueError(
                    f"Unsupported completion option arity: {action.option_strings!r}"
                )
            if action.help != argparse.SUPPRESS:
                menus[index].extend(action.option_strings)
            keys = [f"{index}:{option}" for option in action.option_strings]
            if action.nargs == 0:
                transitions.append((keys, "flag", ""))
            else:
                key = keys[0]
                transitions.append((keys, "value", key))
                values[key] = (
                    [str(value) for value in action.choices]
                    if action.choices is not None and action.help != argparse.SUPPRESS
                    else []
                )
                for option in action.option_strings:
                    if option.startswith("--"):
                        transitions.append(([f"{index}:{option}="], "inline", ""))
                    elif len(option) == 2:
                        transitions.append(([f"{index}:{option}"], "attached", ""))
        install_add = current.get_default("_install_add_command")
        if install_add is not None:
            positional_choices.append((index, 0, [install_add]))

    quote = shlex.quote

    def words(items: list[str]) -> str:
        return " ".join(quote(item) for item in items)

    if shell in {"bash", "zsh"}:
        lines = [
            "_molt_candidates() {",
            "  local node=0 position=0 pending='' word",
            "  _molt_matches=()",
            '  for word in "$@"; do',
            '    if [[ -n "$pending" ]]; then pending=""; continue; fi',
            '    if [[ "$word" == -- ]]; then return; fi',
            '    case "$node:$word" in',
        ]
        for keys, kind, target in transitions:
            pattern = "|".join(quote(key) for key in keys)
            if kind == "child":
                body = f"node={target}; position=0"
            elif kind == "value":
                body = f"pending={quote(target)}"
            else:
                body = ":"
                if kind == "inline":
                    pattern += "*"
                elif kind == "attached":
                    pattern += "?*"
            lines.append(f"      {pattern}) {body} ;;")
        lines.extend(
            [
                "      *)",
                '        if [[ "$word" == -* ]]; then return; fi',
                "        position=$((position + 1))",
                '        case "$node" in',
            ]
        )
        for index, edges in children.items():
            if edges:
                lines.append(f"          {index}) return ;;")
            elif index in remainders:
                lines.append(
                    f"          {index}) if (( position >= {remainders[index]} )); "
                    "then return; fi ;;"
                )
        lines.extend(
            [
                "        esac ;;",
                "    esac",
                "  done",
                '  if [[ -n "$pending" ]]; then',
                '    case "$pending" in',
            ]
        )
        for key, choices in values.items():
            if choices:
                lines.append(f"      {quote(key)}) _molt_matches=({words(choices)}) ;;")
        lines.extend(["    esac", "    return", "  fi", '  case "$node" in'])
        for index, candidates in menus.items():
            lines.append(f"    {index}) _molt_matches=({words(candidates)}) ;;")
        lines.extend(["  esac", '  case "$node:$position" in'])
        for index, position, choices in positional_choices:
            lines.append(
                f"    {quote(f'{index}:{position}')}) "
                f"_molt_matches+=({words(choices)}) ;;"
            )
        lines.extend(["  esac", "}"])
        if shell == "bash":
            # Bash supplies quoted fragments; align them literally rather than
            # parse or evaluate the line. Shell syntax never becomes data.
            # The callback's $2 ends at the cursor; COMP_WORDS can include its
            # suffix, or COMP_CWORD can point at an adjacent separator. The
            # literal cursor prefix locates the unfinished argument without
            # interpreting shell syntax. Before Bash 4.3, COMP_POINT counts
            # bytes; later Bash counts locale characters.
            # https://www.gnu.org/software/bash/manual/html_node/Bash-Variables.html
            lines.extend(
                [
                    "_molt_complete() {",
                    "  local cur candidate word tail gap data_breaks character separator previous_separator=0 last i j adjacent word_start word_prefix current_start line_prefix",
                    "  local line=$COMP_LINE",
                    "  local -a _molt_matches _molt_words",
                    "  COMPREPLY=()",
                    "  _molt_words=()",
                    '  cur="$2"',
                    "  if (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 3) )); then",
                    "    line_prefix=${COMP_LINE:0:COMP_POINT}",
                    "  else",
                    '    printf -v line_prefix \'%.*s\' "$COMP_POINT" "$COMP_LINE"',
                    "  fi",
                    '  [[ "$line_prefix" == *"$cur" ]] || return 0',
                    "  current_start=$((${#line_prefix} - ${#cur}))",
                    "  data_breaks=$COMP_WORDBREAKS",
                    "  for character in ' ' $'\\t' $'\\n' '(' ')' '<' '>' ';' '&' '|' '\"' \"'\" '\\'; do",
                    '    data_breaks=${data_breaks//"$character"/}',
                    "  done",
                    "  for ((i=0; i<=COMP_CWORD; i++)); do",
                    '    word="${COMP_WORDS[i]}"',
                    "    separator=0",
                    '    if [[ -n "$word" ]]; then',
                    "      separator=1",
                    "      for ((j=0; j<${#word}; j++)); do",
                    '        [[ "$data_breaks" == *"${word:j:1}"* ]] || { separator=0; break; }',
                    "      done",
                    '      tail=${line#*"$word"}',
                    '      [[ "$tail" != "$line" ]] || return 0',
                    '      gap=${line%"$word$tail"}',
                    "      line=$tail",
                    "    else",
                    "      gap=$line",
                    "    fi",
                    "    adjacent=0",
                    '    if ((${#_molt_words[@]} && (separator || previous_separator))) && [[ -z "$gap" ]]; then adjacent=1; fi',
                    "    word_start=$((${#COMP_LINE} - ${#line} - ${#word}))",
                    "    if ((i == COMP_CWORD || word_start + ${#word} > current_start)); then",
                    "      ((adjacent)) && return 0",
                    "      word_prefix=${line_prefix:word_start}",
                    '      [[ "$word_prefix" == --*=* ]] && return 0',
                    "      break",
                    "    fi",
                    "    if ((adjacent)); then",
                    "      last=$((${#_molt_words[@]} - 1))",
                    '      _molt_words[last]="${_molt_words[last]}$word"',
                    "    else",
                    '      _molt_words+=("$word")',
                    "    fi",
                    "    previous_separator=$separator",
                    "  done",
                    '  _molt_candidates "${_molt_words[@]:1}"',
                    '  for candidate in "${_molt_matches[@]}"; do',
                    '    [[ "$candidate" == "$cur"* ]] && COMPREPLY+=("$candidate")',
                    "  done",
                    "  return 0",
                    "}",
                    "complete -F _molt_complete molt",
                ]
            )
        else:
            lines[:0] = ["#compdef molt"]
            lines.extend(
                [
                    "_molt() {",
                    "  local -a _molt_matches",
                    '  _molt_candidates "${words[@]:1:CURRENT-2}"',
                    '  compadd -- "${_molt_matches[@]}"',
                    "}",
                    "compdef _molt molt",
                ]
            )
        return "\n".join(lines) + "\n"

    # Fish's argument producer returns exact tokens. A short flag stays '-r';
    # subcommands/positional literals never become fabricated '-l' options.
    lines = [
        "function __molt_candidates",
        "  set -l node 0",
        "  set -l position 0",
        "  set -l pending ''",
        "  set -l tokens (commandline -opc)",
        "  set -e tokens[1]",
        "  for word in $tokens",
        "    if test -n \"$pending\"; set pending ''; continue; end",
        '    if test "$word" = --; return; end',
        '    switch "$node:$word"',
    ]
    for keys, kind, target in transitions:
        patterns = keys
        if kind == "inline":
            patterns = [key + "*" for key in keys]
        elif kind == "attached":
            patterns = [key + "?*" for key in keys]
        lines.append("      case " + words(patterns))
        if kind == "child":
            lines.append(f"        set node {target}; set position 0")
        elif kind == "value":
            lines.append(f"        set pending {quote(target)}")
        else:
            lines.append("        continue")
    lines.extend(
        [
            "      case '*'",
            "        if string match -q -- '-*' \"$word\"; return; end",
            "        set position (math $position + 1)",
            "        switch $node",
        ]
    )
    for index, edges in children.items():
        if edges:
            lines.extend([f"          case {index}", "            return"])
        elif index in remainders:
            lines.extend(
                [
                    f"          case {index}",
                    f"            if test $position -ge {remainders[index]}; return; end",
                ]
            )
    lines.extend(
        [
            "        end",
            "    end",
            "  end",
            '  if test -n "$pending"',
            "    switch $pending",
        ]
    )
    for key, choices in values.items():
        if choices:
            lines.extend(
                [f"      case {quote(key)}", f"        printf '%s\\n' {words(choices)}"]
            )
    lines.extend(["    end", "    return", "  end", "  switch $node"])
    for index, candidates in menus.items():
        if candidates:
            lines.extend(
                [f"    case {index}", f"      printf '%s\\n' {words(candidates)}"]
            )
    lines.extend(["  end", '  switch "$node:$position"'])
    for index, position, choices in positional_choices:
        lines.extend(
            [
                f"    case {quote(f'{index}:{position}')}",
                f"      printf '%s\\n' {words(choices)}",
            ]
        )
    lines.extend(["  end", "end", "complete -c molt -f -a '(__molt_candidates)'"])
    return "\n".join(lines) + "\n"
