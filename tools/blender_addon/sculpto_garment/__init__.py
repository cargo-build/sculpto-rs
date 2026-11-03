"""Sculpto Garment — dress a character from a raw Tripo3D asset.

bl_info must stay at the top: it is what Blender reads to register the
add-on. The panel exposes the two-step product flow: pick the raw garment
file, press Dress. Audit renders are one more press.
"""

bl_info = {
    "name": "Sculpto Garment",
    "author": "Sculpto",
    "version": (0, 1, 0),
    "blender": (4, 2, 0),
    "location": "View3D > Sidebar > Sculpto",
    "description": "Dress a character from a raw Tripo3D garment asset: reconstruct, texture, and rig in one press",
    "category": "Object",
}

import bpy

from . import pipeline

if "bpy" in locals():
    import importlib

    importlib.reload(pipeline)


class SCULPTO_OT_dress(bpy.types.Operator):
    """Reconstruct the raw garment asset onto the active character"""

    bl_idname = "sculpto_garment.dress"
    bl_label = "Dress character from raw asset"
    bl_options = {"REGISTER", "UNDO"}

    filepath: bpy.props.StringProperty(subtype="FILE_PATH")  # noqa: F821
    kind: bpy.props.EnumProperty(  # noqa: F821
        items=[
            ("upper-body", "Upper body", "Tunic, shirt, bodice"),
            ("trousers", "Trousers", "Waist-to-ankle legs"),
            ("footwear", "Footwear", "Boots and shoes (left foot)"),
        ],
        default="upper-body",
    )
    clip: bpy.props.StringProperty(name="Clip", default="Combat_Idle")  # noqa: F821

    def execute(self, context):
        body = context.scene.sculpto_body_pointer
        if body is None or body.type != "MESH":
            # Fall back to the largest mesh that is not the new garment.
            body = next(
                (
                    o
                    for o in reversed(context.scene.objects)
                    if o.type == "MESH" and o.name.startswith("Base")
                ),
                None,
            )
        if body is None:
            self.report({"ERROR"}, "no body mesh: select the character body first")
            return {"CANCELLED"}
        armature = next((o for o in context.scene.objects if o.type == "ARMATURE"), None)
        posed = pipeline.posed_body(body, armature, self.clip)
        raw, material = pipeline.import_raw(self.filepath)
        hem_z, collar_z, width = pipeline.place_raw(raw, posed, self.kind, width_reference=body)
        faces = pipeline.select_region(posed, hem_z, collar_z, kind=self.kind)
        if not faces:
            self.report({"ERROR"}, "no body region matched the garment class")
            return {"CANCELLED"}
        garment = pipeline.build_garment(posed, faces)
        textured = pipeline.project_texture(garment, raw, material)
        if armature is not None:
            pipeline.wire_animation(garment, armature, posed, self.clip)
        posed_mesh = posed.data
        bpy.data.objects.remove(posed, do_unlink=True)
        bpy.data.meshes.remove(posed_mesh)
        # The raw mesh stays in the scene as the appearance source, hidden
        # from both the viewport and renders — a viewport-only hide leaves a
        # floating flat garment in every audit render.
        raw.hide_set(True)
        raw.hide_render = True
        self.report(
            {"INFO"},
            f"dressed {body.name}: {len(garment.data.polygons)} faces, coverage {hem_z:.2f}-{collar_z:.2f} m, width {width:.2f}, textured={textured}",
        )
        return {"FINISHED"}

    def invoke(self, context, event):
        context.window_manager.fileselect_add(self)
        return {"RUNNING_MODAL"}


class SCULPTO_OT_audit(bpy.types.Operator):
    """Render auditor screenshots of the dressed character across a clip"""

    bl_idname = "sculpto_garment.audit"
    bl_label = "Render audit screenshots"
    bl_options = {"REGISTER"}

    directory: bpy.props.StringProperty(subtype="DIR_PATH")  # noqa: F821
    clip: bpy.props.StringProperty(name="Clip", default="Combat_Idle")  # noqa: F821
    frames: bpy.props.StringProperty(name="Frames", default="1,20,40,60,80,100")  # noqa: F821
    views: bpy.props.StringProperty(name="Views", default="front,side,back")  # noqa: F821

    def execute(self, context):
        body = context.scene.sculpto_body_pointer
        garment = context.scene.objects.get("SculptoGarment")
        if body is None or garment is None:
            self.report({"ERROR"}, "need a body and a dressed SculptoGarment")
            return {"CANCELLED"}
        frames = [int(f) for f in self.frames.split(",") if f.strip()]
        views = [v.strip() for v in self.views.split(",") if v.strip()]
        written = pipeline.render_audit(
            body, self.clip, self.directory, frames, views, kind=context.scene.sculpto_garment_kind
        )
        self.report({"INFO"}, f"rendered {len(written)} audit frames")
        return {"FINISHED"}

    def invoke(self, context, event):
        context.window_manager.fileselect_add(self)
        return {"RUNNING_MODAL"}


class SCULPTO_PT_panel(bpy.types.Panel):
    bl_label = "Sculpto Garment"
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "Sculpto"

    def draw(self, context):
        layout = self.layout
        layout.prop(context.scene, "sculpto_body_pointer", text="Body")
        props = layout.operator(SCULPTO_OT_dress.bl_idname, icon="MOD_CLOTH")
        props.clip = "Combat_Idle"
        audit = layout.operator(SCULPTO_OT_audit.bl_idname, icon="RENDER_ANIMATION")
        audit.clip = "Combat_Idle"


CLASSES = (SCULPTO_OT_dress, SCULPTO_OT_audit, SCULPTO_PT_panel)


def register():
    for cls in CLASSES:
        bpy.utils.register_class(cls)
    bpy.types.Scene.sculpto_garment_kind = bpy.props.EnumProperty(
        items=[
            ("upper-body", "Upper body", "Tunic, shirt, bodice"),
            ("trousers", "Trousers", "Waist-to-ankle legs"),
            ("footwear", "Footwear", "Boots and shoes"),
        ],
        default="upper-body",
    )
    bpy.types.Scene.sculpto_body_pointer = bpy.props.PointerProperty(
        name="Character body", type=bpy.types.Object, poll=lambda _s, o: o.type == "MESH"
    )


def unregister():
    for cls in reversed(CLASSES):
        bpy.utils.unregister_class(cls)
    del bpy.types.Scene.sculpto_body_pointer


if __name__ == "__main__":
    register()