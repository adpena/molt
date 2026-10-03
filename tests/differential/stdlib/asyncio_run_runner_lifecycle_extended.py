"""Draft extension of asyncio_run_runner_lifecycle.py; independent CPython oracle."""
import asyncio

log = []

async def background():
    try:
        log.append('started')
        await asyncio.Event().wait()
    finally:
        await asyncio.sleep(0)
        log.append('cleaned')

async def start():
    task = asyncio.create_task(background())
    await asyncio.sleep(0)
    return task

async def fail():
    raise ValueError('main-failure')

runner = asyncio.Runner()
loop = runner.get_loop()
task = runner.run(start())
print('after-start', task.done(), log)
try:
    runner.run(fail())
except ValueError as error:
    print('main-error', str(error))
print('after-error', task.done(), log)
print('next-run', runner.run(asyncio.sleep(0, result='again')))
runner.close()
runner.close()
print('closed', task.cancelled(), loop.is_closed(), log)
try:
    runner.get_loop()
except RuntimeError as error:
    print('closed-get-loop', str(error))

outside_loop = asyncio.new_event_loop()
asyncio.set_event_loop(outside_loop)
with asyncio.Runner(loop_factory=asyncio.new_event_loop) as custom:
    print('factory-policy', asyncio.get_event_loop() is outside_loop)
    print('factory-run', custom.run(asyncio.sleep(0, result='custom')))
print('factory-policy-after', asyncio.get_event_loop() is outside_loop)
asyncio.set_event_loop(None)
outside_loop.close()
