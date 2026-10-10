"""Purpose: differential coverage for unittest.main() command-line parsing.

`unittest.main()` builds its argparse parsers with `parents=`, `store_const`,
`store_true`, `append` with a `type` converter, `nargs='*'` and
`parse_args(argv, namespace)`, so this exercises those argparse paths too.
"""

import contextlib
import io
import unittest


class Sample(unittest.TestCase):
    def test_alpha(self):
        self.assertEqual(1 + 1, 2)

    def test_beta(self):
        self.assertTrue(True)

    @unittest.skip("skipped on purpose")
    def test_gamma(self):
        raise AssertionError("not run")


class Failing(unittest.TestCase):
    def test_fails(self):
        self.assertEqual("a", "b")


def run(argv):
    stdout = io.StringIO()
    stderr = io.StringIO()
    exit_code = None
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        try:
            # A fresh loader per run: `-k` sets `testNamePatterns` on the loader,
            # and the default loader is shared across runs.
            program = unittest.main(
                module="__main__",
                argv=argv,
                exit=False,
                testLoader=unittest.TestLoader(),
            )
        except SystemExit as exc:
            exit_code = exc.code
    if exit_code is not None:
        print(argv[1:], "exit", exit_code)
        return stdout.getvalue(), stderr.getvalue()
    result = program.result
    print(
        argv[1:],
        result.testsRun,
        len(result.failures),
        len(result.errors),
        len(result.skipped),
        result.wasSuccessful(),
    )
    print(
        "  options",
        program.verbosity,
        program.failfast,
        program.buffer,
        program.tb_locals,
        program.durations,
        program.testNamePatterns,
    )
    return stdout.getvalue(), stderr.getvalue()


run(["prog"])
run(["prog", "-v", "Sample"])
run(["prog", "--quiet", "-k", "alpha", "Sample"])
run(["prog", "--failfast", "-b", "Failing", "Sample"])
run(["prog", "--locals", "--durations", "2", "Sample.test_beta"])
run(["prog", "-k", "alpha", "-k", "beta"])
run(["prog", "-k", "*_fa*"])
_, err = run(["prog", "--bogus"])
print("unrecognized arguments: --bogus" in err, err.startswith("usage: prog"))
out, _ = run(["prog", "-h"])
print(out.startswith("usage: prog"), "--failfast" in out, "-k TESTNAMEPATTERNS" in out)
