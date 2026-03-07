from email.message import EmailMessage

msg = EmailMessage()
msg['Subject'] = 'Test Subject'
msg['From'] = 'sender@example.com'
msg['To'] = 'recipient@example.com'
msg.set_content('Hello, this is a test email.')
print(msg['Subject'])
print(msg['From'])
print(msg['To'])
print(msg.get_content_type())
print(msg.get_body().get_content())
