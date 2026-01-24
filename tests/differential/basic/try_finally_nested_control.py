"""Purpose: differential coverage for nested try/finally control precedence."""


def nested_returns():
    try:
        try:
            return "inner"
        finally:
            return "inner_finally"
    finally:
        return "outer_finally"


def nested_break_continue():
    out = []
    for i in range(2):
        try:
            try:
                if i == 0:
                    continue
                break
            finally:
                out.append(f"inner_finally{i}")
        finally:
            out.append(f"outer_finally{i}")
    return out


print("nested_return", nested_returns())
print("nested_loop", nested_break_continue())
