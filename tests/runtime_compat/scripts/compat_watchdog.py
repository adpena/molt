import watchdog
from watchdog.events import FileSystemEventHandler, FileCreatedEvent

print("watchdog", watchdog.__version__)

handler = FileSystemEventHandler()
event = FileCreatedEvent("/tmp/test.txt")
print("event type:", event.event_type)
print("event path:", event.src_path)
print("handler type:", type(handler).__name__)
