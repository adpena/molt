"""Purpose: differential coverage for PEP 657 error location metadata."""


def boom(x: int) -> float:
    return (1 + x) / (x - x)


try:
    boom(1)
except Exception as exc:
    tb = exc.__traceback__
    while tb is not None and tb.tb_next is not None:
        tb = tb.tb_next
    if tb is None:
        print(None, None, None, None)
    else:
        positions = list(tb.tb_frame.f_code.co_positions())
        idx = tb.tb_lasti // 2
        lineno, end_lineno, colno, end_colno = positions[idx]
        print(lineno, end_lineno, colno, end_colno)
