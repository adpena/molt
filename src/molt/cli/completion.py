from __future__ import annotations


def _completion_script(shell: str) -> str:
    commands = [
        "build",
        "extension",
        "factgraph",
        "check",
        "run",
        "repl",
        "compare",
        "parity-run",
        "test",
        "diff",
        "bench",
        "profile",
        "lint",
        "doctor",
        "package",
        "publish",
        "verify",
        "deps",
        "vendor",
        "clean",
        "config",
        "completion",
    ]
    extension_subcommands = ["build", "audit", "scan"]
    extension_options = {
        "build": [
            "--project",
            "--out-dir",
            "--molt-abi",
            "--target",
            "--capabilities",
            "--deterministic",
            "--no-deterministic",
            "--json",
            "--verbose",
        ],
        "audit": [
            "--path",
            "--require-capabilities",
            "--require-abi",
            "--require-checksum",
            "--json",
            "--verbose",
        ],
    }
    options = {
        "build": [
            "--module",
            "--target",
            "--codec",
            "--type-hints",
            "--fallback",
            "--type-facts",
            "--python-version",
            "--pgo-profile",
            "--output",
            "--out-dir",
            "--sysroot",
            "--emit",
            "--emit-ir",
            "--profile",
            "--platform",
            "--build-profile",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--portable",
            "--trusted",
            "--no-trusted",
            "--capabilities",
            "--capability-manifest",
            "--cache",
            "--no-cache",
            "--cache-dir",
            "--cache-report",
            "--rebuild",
            "--respect-pythonpath",
            "--no-respect-pythonpath",
            "--json",
            "--verbose",
        ],
        "check": [
            "--output",
            "--strict",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--json",
            "--verbose",
        ],
        "run": [
            "--module",
            "--build-arg",
            "--python-version",
            "--profile",
            "--build-profile",
            "--rebuild",
            "--timing",
            "--capabilities",
            "--capability-manifest",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "repl": [
            "--capabilities",
            "--io-mode",
            "--molt-cmd",
            "--timeout-sec",
        ],
        "compare": [
            "--python",
            "--python-version",
            "--module",
            "--build-arg",
            "--profile",
            "--build-profile",
            "--rebuild",
            "--capabilities",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "parity-run": [
            "--python",
            "--python-version",
            "--module",
            "--timing",
            "--json",
            "--verbose",
        ],
        "test": [
            "--suite",
            "--python-version",
            "--profile",
            "--build-profile",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "diff": [
            "--python-version",
            "--profile",
            "--build-profile",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "bench": ["--wasm", "--script", "--json", "--verbose"],
        "profile": ["--json", "--verbose"],
        "lint": ["--json", "--verbose"],
        "doctor": ["--strict", "--json", "--verbose"],
        "package": [
            "--output",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--capabilities",
            "--sbom",
            "--no-sbom",
            "--sbom-output",
            "--sbom-format",
            "--signature",
            "--signature-output",
            "--sign",
            "--no-sign",
            "--signer",
            "--signing-key",
            "--signing-identity",
            "--json",
            "--verbose",
        ],
        "publish": [
            "--registry",
            "--registry-token",
            "--registry-user",
            "--registry-password",
            "--registry-timeout",
            "--dry-run",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--capabilities",
            "--require-signature",
            "--no-require-signature",
            "--verify-signature",
            "--no-verify-signature",
            "--trusted-signers",
            "--signer",
            "--signing-key",
            "--json",
            "--verbose",
        ],
        "verify": [
            "--package",
            "--manifest",
            "--artifact",
            "--require-checksum",
            "--extension-metadata",
            "--no-extension-metadata",
            "--require-extension-capabilities",
            "--require-extension-abi",
            "--require-deterministic",
            "--require-signature",
            "--no-require-signature",
            "--verify-signature",
            "--no-verify-signature",
            "--trusted-signers",
            "--signer",
            "--signing-key",
            "--capabilities",
            "--json",
            "--verbose",
        ],
        "factgraph": [
            "--module",
            "--output",
            "--target",
            "--backend",
            "--profile",
            "--type-hints",
            "--fallback",
            "--python-version",
            "--capabilities",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "deps": ["--include-dev", "--json", "--verbose"],
        "install": [
            "-r",
            "--requirements",
            "--sync",
            "--json",
            "--verbose",
            "add",
        ],
        "vendor": [
            "--include-dev",
            "--output",
            "--dry-run",
            "--allow-non-tier-a",
            "--extras",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--json",
            "--verbose",
        ],
        "clean": [
            "--apply",
            "--kill-processes",
            "--extra-path",
            "--list-paths",
            "--json",
            "--verbose",
        ],
        "config": ["--file", "--json", "--verbose"],
        "completion": ["--shell", "--json", "--verbose"],
    }
    if shell == "bash":
        lines = [
            "_molt_complete() {",
            "  local cur prev",
            "  COMPREPLY=()",
            '  cur="${COMP_WORDS[COMP_CWORD]}"',
            '  prev="${COMP_WORDS[COMP_CWORD-1]}"',
            "  if [[ ${COMP_CWORD} -eq 1 ]]; then",
            f'    COMPREPLY=( $(compgen -W "{" ".join(commands)}" -- "$cur") )',
            "    return 0",
            "  fi",
            '  if [[ "${COMP_WORDS[1]}" == "extension" ]]; then',
            "    if [[ ${COMP_CWORD} -eq 2 ]]; then",
            '      COMPREPLY=( $(compgen -W "build audit" -- "$cur") )',
            "      return 0",
            "    fi",
            '    case "${COMP_WORDS[2]}" in',
        ]
        for sub in extension_subcommands:
            opts = " ".join(extension_options.get(sub, []))
            lines.append(f'      {sub}) opts="{opts}" ;;')
        lines.extend(
            [
                '      *) opts="" ;;',
                "    esac",
                '    COMPREPLY=( $(compgen -W "$opts" -- "$cur") )',
                "    return 0",
                "  fi",
                '  case "${COMP_WORDS[1]}" in',
            ]
        )
        for cmd in commands:
            opts = " ".join(options.get(cmd, []))
            lines.append(f'    {cmd}) opts="{opts}" ;;')
        lines.extend(
            [
                '    *) opts="" ;;',
                "  esac",
                '  COMPREPLY=( $(compgen -W "$opts" -- "$cur") )',
                "}",
                "complete -F _molt_complete molt",
            ]
        )
        return "\n".join(lines) + "\n"
    if shell == "zsh":
        lines = [
            "#compdef molt",
            "_molt() {",
            "  local -a commands",
            f"  commands=({' '.join(commands)})",
            "  if (( CURRENT == 2 )); then",
            "    compadd $commands",
            "    return",
            "  fi",
            "  if [[ $words[2] == extension ]]; then",
            "    if (( CURRENT == 3 )); then",
            "      compadd build audit",
            "      return",
            "    fi",
            "    local -a extension_opts",
            "    case $words[3] in",
        ]
        for sub in extension_subcommands:
            opts = " ".join(extension_options.get(sub, []))
            lines.append(f"      {sub}) extension_opts=({opts}) ;;")
        lines.extend(
            [
                "      *) extension_opts=() ;;",
                "    esac",
                "    compadd $extension_opts",
                "    return",
                "  fi",
                "  local -a opts",
                "  case $words[2] in",
            ]
        )
        for cmd in commands:
            opts = " ".join(options.get(cmd, []))
            lines.append(f"    {cmd}) opts=({opts}) ;;")
        lines.extend(
            [
                "    *) opts=() ;;",
                "  esac",
                "  compadd $opts",
                "}",
                "compdef _molt molt",
            ]
        )
        return "\n".join(lines) + "\n"
    if shell == "fish":
        lines = [
            f"complete -c molt -f -n '__fish_use_subcommand' -a \"{' '.join(commands)}\"",
            "complete -c molt -f -n '__fish_seen_subcommand_from extension; and not __fish_seen_subcommand_from build audit' -a \"build audit\"",
        ]
        for cmd in commands:
            for opt in options.get(cmd, []):
                opt_name = opt.lstrip("-")
                lines.append(
                    f"complete -c molt -n '__fish_seen_subcommand_from {cmd}' -l {opt_name}"
                )
        for sub in extension_subcommands:
            for opt in extension_options.get(sub, []):
                opt_name = opt.lstrip("-")
                lines.append(
                    "complete -c molt "
                    "-n '__fish_seen_subcommand_from extension; and "
                    f"__fish_seen_subcommand_from {sub}' -l {opt_name}"
                )
        return "\n".join(lines) + "\n"
    raise ValueError(f"Unsupported shell: {shell}")
