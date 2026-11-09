"""GLB post-write sanitizers: thin shells over the ``sculpto`` command surface.

Blender 5.2 writes ``"texCoord": -1`` for material textures whose UV map does
not resolve on a mesh, and samples a non-zero default ``mesh.weights`` from the
current frame. Engines reject the first (an unsigned set index is expected) and
apply every morph target for the second. Both repairs are command ids in the
Rust adapter (``gltf.clamp_texcoord`` and ``gltf.reset_weights``), so this
module only forwards the path through ``cli_bridge`` and edits no glTF JSON.

Usage (after ``bpy.ops.export_scene.gltf`` wrote ``out_path``):

    from glb_sanitize import reset_mesh_weights, sanitize_texcoord
    reset_mesh_weights(out_path)
    sanitize_texcoord(out_path)
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "blender_addon" / "sculpto_garment"))
from cli_bridge import CommandError, EnvelopeError, SculptoCli


def repair(command: str, out_path: str) -> dict:
    """Run one repair on ``out_path`` in place and return its result member."""
    try:
        return SculptoCli().run(command, {"glb": str(out_path)})
    except (CommandError, EnvelopeError) as error:
        raise SystemExit(f"{command}: {error}") from error


def sanitize_texcoord(out_path: str) -> int:
    """Clamp every ``texCoord: -1`` to UV set 0 in place; return the count."""
    return repair("gltf.clamp_texcoord", out_path)["texcoords"]


def reset_mesh_weights(out_path: str) -> int:
    """Zero every mesh's default morph weights in place; return the count."""
    return repair("gltf.reset_weights", out_path)["meshes"]
