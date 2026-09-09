import json
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import threading


def reply(listener, errors):
    try:
        connection, _ = listener.accept()
        with connection:
            connection.settimeout(10)
            header = connection.recv(8, socket.MSG_WAITALL)
            assert header[:4] == b"LNS2", "invalid wire magic"
            size, = struct.unpack(">I", header[4:])
            assert 1 < size <= 1_048_576, "invalid frame size"
            body = connection.recv(size, socket.MSG_WAITALL)
            assert body[0] == 1, "invalid wire subtype"
            request = json.loads(body[1:])
            assert request["type"] == "InspectApprovalOffer"
            payload = b'\x01{"type":"ApprovalOffer","offer":null}'
            connection.sendall(b"LNS2" + struct.pack(">I", len(payload)) + payload)
    except Exception as error:
        errors.append(error)


with tempfile.TemporaryDirectory(prefix="lns-wire-", dir="/tmp") as directory:
    address = str(Path(directory) / "service.sock")
    with socket.socket(socket.AF_UNIX) as listener:
        listener.bind(address)
        listener.listen()
        listener.settimeout(15)
        errors = []
        worker = threading.Thread(target=reply, args=(listener, errors), daemon=True)
        worker.start()
        result = subprocess.run([sys.argv[1], "--probe", address], capture_output=True, text=True, timeout=12)
        if result.returncode != 0:
            errors = [line for line in result.stderr.splitlines() if "Fatal error" in line or "Error raised" in line]
            raise RuntimeError("local socket probe: " + " | ".join(errors or result.stderr.splitlines()[:8]))
        worker.join(timeout=1)
        assert not worker.is_alive(), "local server did not complete the exchange"
        assert not errors, errors
        print(result.stdout, end="")
