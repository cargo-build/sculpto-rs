"""Headless cloth-bake entry point for one Sculpto request.

Blender runs this script:

    blender --background --factory-startup <scene.blend> --python-exit-code 1 \
        --python tools/blender_cloth/run_request.py -- \
        --request <out>/request.json --result <out>/result.json

The script validates the request identity, adopts ``tools/cloth_bake.py`` as the
solver body, and writes the versioned result. A refusal is a result with
``status: "refused"`` and a non-zero exit; a crash without a result is an
environment failure for the Rust caller.

No format rule or solver value is invented here: the request carries every
knob, and the adopted bake owns simulation and OBJ writing.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import runpy
import sys
import time
import traceback
from pathlib import Path

import bpy

SCHEMA = "sculpto.blender.cloth_bake"
VERSION = 2
RESULT_SCHEMA = "sculpto.blender.cloth_bake.result"
RESULT_VERSION = 2


class Refusal(Exception):
    """A named refusal that still produces a result document."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


def parse_arguments() -> argparse.Namespace:
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--request", required=True)
    parser.add_argument("--result", required=True)
    return parser.parse_args(argv)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def version_matches(actual: str, required: str) -> bool:
    def parts(text: str) -> list[int]:
        values: list[int] = []
        for chunk in text.split("."):
            match = re.match(r"\d+", chunk)
            if not match:
                break
            values.append(int(match.group(0)))
        return values

    left, right = parts(actual), parts(required)
    return bool(right) and len(right) <= 3 and len(left) >= len(right) and left[: len(right)] == right


def artifact(out: Path, path: Path) -> dict:
    relative = path.relative_to(out)
    if ".." in relative.parts:
        raise Refusal("artifact_path", f"artifact escapes the output directory: {path}")
    data = path.read_bytes()
    return {
        "path": relative.as_posix(),
        "sha256": sha256_bytes(data),
        "bytes": len(data),
    }


def obj_counts(path: Path) -> tuple[int, int]:
    vertices = 0
    triangles = 0
    with path.open("rb") as source:
        for line in source:
            if line.startswith(b"v "):
                vertices += 1
            elif line.startswith(b"f "):
                triangles += len(line.split()) - 3
    return vertices, triangles


def write_result(path: Path, result: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(result, indent=1) + "\n", encoding="utf-8")


def solver_arguments(request: dict) -> tuple[str, list[str]]:
    simulation = request["simulation"]
    pin = simulation["pin"]
    common = {
        "garment": request["garment"]["path"],
        "out": request["output"]["directory"],
        "clip": request["scene"]["clip"],
        "body": request["scene"]["body_object"],
        "frame_end": simulation["frame_end"],
        "frame_end_mode": simulation["frame_end_mode"],
        "sample": simulation["sample_stride"],
        "body_frames": 1 if request["output"]["body_frames"] else 0,
    }
    if simulation["solver"] == "xpbd":
        xpbd = simulation["xpbd"]
        policy = pin["policy"]
        if policy == "shoulder":
            pin_mode = "above"
        else:
            pin_mode = "auto"
        common.update(
            {
                "settle": simulation["settle_frames"],
                "substeps": xpbd["substeps"],
                "constraint_steps": xpbd["constraint_steps"],
                "pin_height": pin["height_fraction"],
                "pin_mode": pin_mode,
                "pin_band": pin["band_m"],
                "pin_contact": pin["contact_m"],
                "pin_all": 1 if pin.get("pin_all") else 0,
                "stretchiness": xpbd["stretchiness"],
                "bendiness": xpbd["bendiness"],
                "linear_damping": xpbd["linear_damping"],
                "gravity": 1 if xpbd["gravity"] else 0,
                "collide": 1 if xpbd["collide"] else 0,
                "collider_friction": xpbd["collider_friction"],
                "collider_margin": xpbd["collider_margin"],
                "edge_contacts": 1 if xpbd["edge_contacts"] else 0,
                "collision_radius": xpbd["collision_radius"],
            }
        )
        return "cloth_bake.py", [f"{key}={value}" for key, value in common.items()]

    legacy = simulation["legacy"]
    legacy_policy = {"explicit_z": "explicit"}.get(pin["policy"], pin["policy"])
    common.update(
        {
            "bending": legacy["bending"],
            "structural": legacy["structural"],
            "mass": legacy["mass"],
            "self_collision": 1 if legacy["self_collision"] else 0,
            "self_min": legacy["self_min"],
            "body_min": legacy["body_min"],
            "quality": legacy["quality"],
            "ground": 1 if legacy["ground"] else 0,
            "pin_policy": legacy_policy,
            "pin_height": pin["height_fraction"],
            "pin_band": pin["band_m"],
        }
    )
    if pin["policy"] == "explicit_z":
        common["pin_z"] = pin["explicit_z"]
    return "cloth_bake_legacy.py", [f"{key}={value}" for key, value in common.items()]


def run_adopted_bake(request: dict) -> dict:
    script_name, arguments = solver_arguments(request)
    bake_path = Path(__file__).resolve().parents[1] / script_name
    if not bake_path.is_file():
        raise Refusal("bake_script_missing", f"adopted bake script not found: {bake_path}")
    sys.argv = [str(bake_path), "--", *arguments]
    runpy.run_path(str(bake_path), run_name="__main__")
    report_path = Path(request["output"]["directory"]) / "report.json"
    if not report_path.is_file():
        raise Refusal("bake_report_missing", f"bake wrote no report: {report_path}")
    return json.loads(report_path.read_text(encoding="utf-8"))


def pin_records(report: dict, solver: str, policy: str, shells: list[str]) -> list[dict]:
    records = []
    pin = report.get("pin", {})
    levels = pin.get("shells", {})
    for name in shells:
        if solver == "xpbd":
            world_z = levels.get(name, pin.get("world_z"))
            pinned = report["shells"][name].get("pinned", 0)
        else:
            world_z = pin.get("z")
            pinned = pin.get("pinned", 0)
        records.append(
            {
                "shell": name,
                "policy": policy,
                "world_z": world_z,
                "pinned": int(pinned),
            }
        )
    return records


def shell_result(out: Path, name: str, report: dict) -> dict:
    rest_path = out / f"rest_{name}.obj"
    if not rest_path.is_file():
        raise Refusal("bake_artifact_missing", f"rest mesh is missing: {rest_path}")
    vertices, triangles = obj_counts(rest_path)
    reported = int(report["shells"][name]["verts"])
    if vertices != reported:
        raise Refusal(
            "bake_vertex_count",
            f"{rest_path.name}: {vertices} vertices, bake reported {reported}",
        )
    frames = []
    for frame in sorted(int(key) for key in report["frames"]):
        path = out / f"frame_{frame}_{name}.obj"
        if not path.is_file():
            raise Refusal("bake_artifact_missing", f"frame mesh is missing: {path}")
        item = artifact(out, path)
        frames.append(
            {
                "frame": frame,
                "path": item["path"],
                "sha256": item["sha256"],
                "bytes": item["bytes"],
            }
        )
    return {
        "name": name,
        "vertices": vertices,
        "triangles": triangles,
        "rest": artifact(out, rest_path),
        "frames": frames,
    }


def main() -> int:
    started = time.time()
    arguments = parse_arguments()
    request_path = Path(arguments.request).resolve()
    result_path = Path(arguments.result).resolve()
    raw = request_path.read_bytes()
    request = json.loads(raw)
    out = Path(request["output"]["directory"]).resolve()
    runner_path = Path(__file__).resolve()
    runner_sha = sha256_file(runner_path)

    result = {
        "schema": RESULT_SCHEMA,
        "version": RESULT_VERSION,
        "run_id": request.get("run_id", ""),
        "request_sha256": sha256_bytes(raw),
        "status": "refused",
        "host": {
            "path": bpy.app.binary_path,
            "version": bpy.app.version_string,
            "runner_path": str(runner_path),
            "runner_sha256": runner_sha,
        },
        "shells": [],
        "warnings": [],
    }

    try:
        if request.get("schema") != SCHEMA or request.get("version") != VERSION:
            raise Refusal(
                "request_schema",
                f"expected {SCHEMA} v{VERSION}, got {request.get('schema')!r} v{request.get('version')!r}",
            )
        expected_runner = request.get("runner", {}).get("sha256")
        if expected_runner and expected_runner.lower() != runner_sha:
            raise Refusal(
                "runner_digest",
                f"runner digest {runner_sha} does not match request {expected_runner}",
            )
        required = request["blender"]["version"]
        if not version_matches(bpy.app.version_string, required):
            raise Refusal(
                "blender_version",
                f"Blender {bpy.app.version_string} does not satisfy {required}",
            )
        if request["scene"].get("armature_object"):
            raise Refusal(
                "unsupported_armature",
                "armature_object selection is not implemented by the adopted bake; pass null",
            )
        if request.get("profile"):
            raise Refusal(
                "unsupported_profile",
                "host profile resolution is not implemented yet; pass null",
            )
        for label in ("scene", "garment"):
            expected = request[label].get("sha256")
            if expected and sha256_file(Path(request[label]["path"])) != expected.lower():
                raise Refusal("digest_mismatch", f"{label} digest does not match the request")
        out.mkdir(parents=True, exist_ok=True)
        report = run_adopted_bake(request)
        solver = report.get("solver", "")
        shells = [
            shell_result(out, name, report) for name in sorted(report["shells"])
        ]
        sampled = sorted(int(key) for key in report["frames"])
        start, end = (int(value) for value in report["frame_range"])
        settle = int(request["simulation"]["settle_frames"])
        if end < settle:
            raise Refusal(
                "frame_range",
                f"the clip ends at frame {end}, before the {settle}-frame settle window",
            )
        body_frames = []
        if request["output"]["body_frames"]:
            for frame in sampled:
                path = out / f"body_{frame}.obj"
                if not path.is_file():
                    raise Refusal(
                        "body_frame_missing", f"body frame is missing: {path}"
                    )
                item = artifact(out, path)
                body_frames.append(
                    {
                        "frame": frame,
                        "path": item["path"],
                        "sha256": item["sha256"],
                        "bytes": item["bytes"],
                    }
                )
        armature = next(
            (obj.name for obj in bpy.data.objects if obj.type == "ARMATURE"), ""
        )
        result.update(
            {
                "status": "ok",
                "solver": solver,
                "scene": {
                    "body_object": request["scene"]["body_object"],
                    "armature": armature,
                    "clip": request["scene"]["clip"],
                    "frame_start": start,
                    "frame_end": end,
                    "units_per_metre": request["scene"]["units_per_metre"],
                },
                "frames": {
                    "start": start,
                    "end": end,
                    "sampled": sampled,
                    "fps": request["output"]["fps"],
                },
                "shells": shells,
                "body_frames": body_frames,
                "pin": pin_records(
                    report,
                    solver,
                    request["simulation"]["pin"]["policy"],
                    [shell["name"] for shell in shells],
                ),
                "timings": {
                    "simulation_seconds": float(report["sim_seconds"]),
                    "total_seconds": round(time.time() - started, 3),
                },
                "report": artifact(out, out / "report.json"),
            }
        )
        write_result(result_path, result)
        print(f"CLOTH_RESULT ok run={request['run_id']} shells={len(shells)} frames={len(sampled)}")
        return 0
    except Refusal as refusal:
        result["error"] = {"code": refusal.code, "message": refusal.message}
        write_result(result_path, result)
        print(f"CLOTH_REFUSED {refusal.code}: {refusal.message}", file=sys.stderr)
        return 1
    except SystemExit as exit_error:
        message = str(exit_error.code) if exit_error.code is not None else "bake exited"
        result["error"] = {"code": "bake_exit", "message": message}
        write_result(result_path, result)
        print(f"CLOTH_REFUSED bake_exit: {message}", file=sys.stderr)
        return 1
    except Exception as error:  # noqa: BLE001 - the result must exist for every failure
        result["error"] = {
            "code": "simulation_failed",
            "message": f"{type(error).__name__}: {error}",
        }
        result["warnings"].append(traceback.format_exc())
        write_result(result_path, result)
        print(f"CLOTH_FAILED {type(error).__name__}: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
