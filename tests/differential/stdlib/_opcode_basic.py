"""Purpose: differential coverage for _opcode basics."""

import _opcode


def main() -> None:
    print("stats", _opcode.get_specialization_stats())
    print("se_load_const", _opcode.stack_effect(100, 0))
    print("se_return", _opcode.stack_effect(83, None))
    print("se_return_const", _opcode.stack_effect(121, 0))


if __name__ == "__main__":
    main()
