"""Purpose: an escaping KeyboardInterrupt subclass exits with status 1.

CPython marks an unhandled keyboard interrupt only when the escaping
exception's type is exactly KeyboardInterrupt (Python/pythonrun.c
`run_eval_code_obj`), and only then re-delivers SIGINT at exit (Modules/main.c
`exit_sigint`). A subclass takes the ordinary uncaught-exception exit.
"""


class Interrupted(KeyboardInterrupt):
    pass


print("raising", issubclass(Interrupted, KeyboardInterrupt))
raise Interrupted("subclass")
