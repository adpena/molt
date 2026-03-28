# Resource enforcement scenario: infinite loop must be killed by time limit.
# Expected: process terminates within 2s when max_duration=1s.
while True:
    pass
