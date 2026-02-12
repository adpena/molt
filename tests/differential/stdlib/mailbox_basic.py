# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for mailbox basic."""

import os
import tempfile
import mailbox
from email.message import EmailMessage

root = tempfile.mkdtemp()
box_path = os.path.join(root, 'box')
box = mailbox.mbox(box_path, create=True)
msg = EmailMessage()
msg["From"] = "a@example.com"
msg["To"] = "b@example.com"
msg["Subject"] = "hello"
msg.set_content("payload")
box.add(msg)
box.flush()
box.close()
print(os.path.exists(box_path))

box = mailbox.mbox(box_path, create=False)
print(len(box))
first = box[0]
print(first["Subject"])
box.close()
