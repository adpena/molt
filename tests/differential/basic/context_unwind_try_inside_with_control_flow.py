"""Purpose: return exits through ordinary try scopes inside with scopes."""

PATH = "tests/differential/basic/context_unwind_try_inside_with_control_flow.py"

def return_case(flag):
    with open(PATH, "r") as handle:
        try:
            if flag:
                return handle.read(0) == ""
        except ValueError:
            return False
    return False


print("return_result", return_case(True))
