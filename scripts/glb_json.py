"""Dump a .glb's JSON chunk (glTF is a 12-byte header + a JSON chunk + a BIN chunk).

    python3 scripts/glb_json.py <file.glb>        # pretty JSON to stdout
"""
import json, struct, sys

with open(sys.argv[1], "rb") as f:
    magic, ver, _ = struct.unpack("<III", f.read(12))
    assert magic == 0x46546C67, "not a glb"
    clen, ctype = struct.unpack("<II", f.read(8))
    assert ctype == 0x4E4F534A, "first chunk not JSON"
    doc = json.loads(f.read(clen))

print(json.dumps(doc, indent=2))
