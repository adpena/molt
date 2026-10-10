import schedule

print("schedule", schedule.__version__)

results = []


def job():
    results.append("ran")


schedule.every(1).seconds.do(job)
print("jobs scheduled:", len(schedule.get_jobs()))
schedule.clear()
print("jobs after clear:", len(schedule.get_jobs()))
