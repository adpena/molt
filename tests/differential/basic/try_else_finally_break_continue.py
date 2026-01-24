"""Purpose: differential coverage for try/else/finally with break/continue."""


def loop_flags():
    out = []
    for i in range(3):
        try:
            if i == 0:
                continue
            if i == 1:
                break
        except Exception:
            out.append("except")
        else:
            out.append(f"else{i}")
        finally:
            out.append(f"finally{i}")
    return out


print("out", loop_flags())
