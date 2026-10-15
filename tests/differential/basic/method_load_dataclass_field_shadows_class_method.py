"""Purpose: a dataclass field shadows a same-named class method at a method load (HF-216).

CPython looks an instance attribute up before a non-data class attribute, so
`t.action()` calls the field's value when a dataclass field has the method's
name. The per-site method cache must read the same instance tier as the
generic lookup: dataclass fields, reassigned fields and instance-dictionary
attributes all win over the class function, at a cold site and a warm one.
"""

from dataclasses import dataclass, field


class Base:
    def action(self):
        return "Base.action"

    def describe(self):
        return "Base.describe"


@dataclass
class Task(Base):
    action: object


@dataclass
class Job(Base):
    name: str
    action: object = field(default=lambda: "default field")


def run(target):
    return target.action()


def describe(target):
    return target.describe()


print(run(Task(lambda: "field")))
for index in range(3):
    print(run(Task(lambda index=index: f"field {index}")))
print(run(Base()))
print(run(Job("job")))

task = Task(lambda: "first")
print(run(task))
task.action = lambda: "reassigned"
print(run(task))

job = Job("plain", lambda: "given")
print(describe(job), run(job))
job.describe = lambda: "instance attribute"
print(describe(job), describe(Job("other")))

tasks = [Task(lambda index=index: index * index) for index in range(4)]
print([run(item) for item in tasks])
print([run(item) for item in [Base(), Task(lambda: "mixed"), Base()]])
