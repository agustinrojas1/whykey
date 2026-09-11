#!/usr/bin/env python3
"""Small socket2-compatible fixture server; prints its path then accepts lines."""

import os
import socket
import sys
import tempfile

path = sys.argv[1] if len(sys.argv) > 1 else tempfile.mktemp(prefix="whykey-socket2-")
try:
    os.unlink(path)
except FileNotFoundError:
    pass
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(path)
server.listen(1)
print(path, flush=True)
connection, _ = server.accept()
with connection:
    connection.sendall(b"custom>>whykey-probe,wk-fixture,armed\n")
    for line in sys.stdin:
        connection.sendall(line.encode())
server.close()
os.unlink(path)
