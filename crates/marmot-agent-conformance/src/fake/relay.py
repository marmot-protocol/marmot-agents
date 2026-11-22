#!/usr/bin/env python3
"""Stands in for a harness program that a driver starts on standard input and output, such as
`pi --mode rpc`. It connects to the fake harness's Unix socket (the first argument), says which
chat it serves and how it was started, then passes bytes both ways until the fake hangs up. The
fake, in the test program, answers as the harness would. `--version` is answered here."""

import json
import os
import socket
import sys
import threading


def main():
    path, args = sys.argv[1], sys.argv[2:]
    if "--version" in args:
        print("0.0.0-fake")
        return
    sock = socket.socket(socket.AF_UNIX)
    sock.connect(path)
    hello = {"chat": os.environ.get("MARMOT_AGENT_CHAT", ""), "args": args}
    sock.sendall((json.dumps(hello) + "\n").encode())

    def up():
        while True:
            data = os.read(0, 65536)
            if not data:
                break
            sock.sendall(data)
        try:
            sock.shutdown(socket.SHUT_WR)
        except OSError:
            pass

    threading.Thread(target=up, daemon=True).start()
    while True:
        data = sock.recv(65536)
        if not data:
            break
        os.write(1, data)
    os._exit(0)


if __name__ == "__main__":
    main()
