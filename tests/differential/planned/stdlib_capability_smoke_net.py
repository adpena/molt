# MOLT_ENV: MOLT_CAPABILITIES=net.outbound,env.read
"""Purpose: stdlib import smoke for net-capability modules."""

import ftplib
import imaplib
import poplib
import smtplib
import xmlrpc
import webbrowser

modules = [
    ftplib,
    imaplib,
    poplib,
    smtplib,
    xmlrpc,
    webbrowser,
]
print([mod.__name__ for mod in modules])
