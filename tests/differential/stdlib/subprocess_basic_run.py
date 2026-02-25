"""Purpose: differential coverage for subprocess.run, subprocess.PIPE,
subprocess.DEVNULL, subprocess.check_call, subprocess.getstatusoutput,
subprocess.getoutput."""

import subprocess

# run with capture
r = subprocess.run(["echo", "hello"], capture_output=True, text=True)
print("run returncode:", r.returncode)
print("run stdout:", r.stdout.strip())
# PIPE, DEVNULL constants
print("PIPE:", subprocess.PIPE)
print("DEVNULL:", subprocess.DEVNULL)
# check_call
rc = subprocess.check_call(["true"])
print("check_call:", rc)
# getstatusoutput
status, output = subprocess.getstatusoutput("echo hello")
print("getstatusoutput status:", status)
print("getstatusoutput output:", output)
# getoutput
out = subprocess.getoutput("echo world")
print("getoutput:", out)
