import pathlib
import struct
import sys

data = pathlib.Path(sys.argv[1]).read_bytes()
assert data[:4] == b"icns" and struct.unpack(">I", data[4:8])[0] == len(data), "Invalid icon container"
types = set()
offset = 8
while offset < len(data):
    kind, size = struct.unpack(">4sI", data[offset:offset + 8])
    assert size >= 8 and offset + size <= len(data), "Invalid icon entry"
    types.add(kind)
    offset += size
assert {b"ic07", b"ic08", b"ic09", b"ic10"} <= types, "Dock icon is missing 128, 256, 512, or 1024 pixel representations"
print("PASS: Dock icon includes standard and Retina representations")
