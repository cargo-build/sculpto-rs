"""GLB post-write sanitizer shared by the export tools.

Blender 5.2 writes ``"texCoord": -1`` for material textures whose UV map
does not resolve on a mesh; engines reject the GLB (u32 expected). UV set
0 is the only set the cloth shells carry, so clamp it there, at any depth
of the material JSON.

Usage (after ``bpy.ops.export_scene.gltf`` wrote ``out_path``):

    from glb_sanitize import sanitize_texcoord
    sanitize_texcoord(out_path)
"""

import json
import struct


def _clamp(node) -> bool:
    changed = False
    if isinstance(node, dict):
        if node.get("texCoord") == -1:
            node["texCoord"] = 0
            changed = True
        for value in node.values():
            changed |= _clamp(value)
    elif isinstance(node, list):
        for value in node:
            changed |= _clamp(value)
    return changed


def sanitize_texcoord(out_path: str) -> int:
    """Clamp every ``texCoord: -1`` to 0 in-place; return the token count."""
    raw = bytearray(open(out_path, "rb").read())
    magic, version, _total = struct.unpack("<III", raw[:12])
    json_len, json_type = struct.unpack("<II", raw[12:20])
    doc = json.loads(raw[20 : 20 + json_len])
    changed = False
    for material in doc.get("materials", []):
        changed |= _clamp(material)
    if not changed:
        return 0
    payload = json.dumps(doc, separators=(",", ":")).encode()
    payload += b" " * ((-len(payload)) % 4)
    tail = bytes(raw[20 + json_len :])
    with open(out_path, "wb") as sink:
        sink.write(struct.pack("<III", magic, version, 12 + 8 + len(payload) + len(tail)))
        sink.write(struct.pack("<II", len(payload), json_type))
        sink.write(payload)
        sink.write(tail)
    return 1
