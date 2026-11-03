"""Blender scene preparation shared by the cloth export adapters."""

import bpy
from mathutils import Matrix


def shared_animation_root(scene, objects=None):
    """Keep skeletal and morph channels under one identity export root."""
    selected = tuple(scene.objects if objects is None else objects)
    if not selected or any(obj.name not in scene.objects for obj in selected):
        raise ValueError("export objects must be a nonempty selection from the scene")
    included = set(selected)
    if any(obj.parent is not None and obj.parent not in included for obj in selected):
        raise ValueError("export selection must include every parent")
    roots = [obj for obj in selected if obj.parent is None]
    if not roots:
        raise ValueError("export selection has no hierarchy root")
    root = bpy.data.objects.new("SculptoOutfit", None)
    scene.collection.objects.link(root)
    for obj in roots:
        obj.parent = root
        obj.matrix_parent_inverse = Matrix.Identity(4)
    root.select_set(True)
    bpy.context.view_layer.update()
    return root
