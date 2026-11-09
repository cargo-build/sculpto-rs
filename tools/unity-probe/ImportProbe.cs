using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using UnityEditor;
using UnityEngine;

/// <summary>Headless glTF import probe: reports mesh, morph, clip and skinning stats.</summary>
public static class ImportProbe
{
    public static void Run()
    {
        var report = new Dictionary<string, object>();
        AssetDatabase.Refresh();

        // Discover all GLB files in Assets/Garments (fallback to Assets/)
        var searchDir = Directory.Exists("Assets/Garments") ? "Assets/Garments" : "Assets";
        var files = Directory.GetFiles(searchDir, "*.glb", SearchOption.AllDirectories)
            .Select(f => f.Replace('\\', '/'))
            .OrderBy(f => f)
            .ToList();

        foreach (var path in files)
        {
            var assets = AssetDatabase.LoadAllAssetsAtPath(path);
            if (assets == null || assets.Length < 2)
            {
                AssetDatabase.ImportAsset(path, ImportAssetOptions.ForceUpdate);
                assets = AssetDatabase.LoadAllAssetsAtPath(path);
            }

            var meshes = assets.OfType<Mesh>().ToList();
            var entries = new List<Dictionary<string, object>>();
            int totalVerts = 0;
            int totalBlendShapes = 0;
            int totalTriangles = 0;

            foreach (var mesh in meshes)
            {
                int triCount = 0;
                for (int s = 0; s < mesh.subMeshCount; s++)
                {
                    triCount += (int)(mesh.GetIndexCount(s) / 3);
                }
                totalVerts += mesh.vertexCount;
                totalBlendShapes += mesh.blendShapeCount;
                totalTriangles += triCount;

                entries.Add(new Dictionary<string, object>
                {
                    ["name"] = mesh.name,
                    ["vertices"] = mesh.vertexCount,
                    ["triangles"] = triCount,
                    ["blendShapes"] = mesh.blendShapeCount,
                    ["subMeshes"] = mesh.subMeshCount,
                    ["bounds"] = mesh.bounds.size.ToString(),
                    ["bindposes"] = mesh.bindposeCount,
                });
            }

            var gameObjects = assets.OfType<GameObject>().ToList();
            var skinnedRenderers = assets.OfType<SkinnedMeshRenderer>().Count();
            if (skinnedRenderers == 0 && gameObjects.Count > 0)
            {
                skinnedRenderers = gameObjects.SelectMany(go => go.GetComponentsInChildren<SkinnedMeshRenderer>(true)).Distinct().Count();
            }

            var meshRenderers = assets.OfType<MeshRenderer>().Count();
            if (meshRenderers == 0 && gameObjects.Count > 0)
            {
                meshRenderers = gameObjects.SelectMany(go => go.GetComponentsInChildren<MeshRenderer>(true)).Distinct().Count();
            }

            int nodeCount = assets.OfType<Transform>().Count();
            if (nodeCount == 0 && gameObjects.Count > 0)
            {
                nodeCount = gameObjects.SelectMany(go => go.GetComponentsInChildren<Transform>(true)).Distinct().Count();
            }

            var clips = assets.OfType<AnimationClip>().Select(c => c.name + ":" + c.length.ToString("0.00")).ToList();
            var materials = assets.OfType<Material>().Select(m => m.name).ToList();

            report[path] = new Dictionary<string, object>
            {
                ["mesh_count"] = entries.Count,
                ["total_vertices"] = totalVerts,
                ["total_triangles"] = totalTriangles,
                ["total_blend_shapes"] = totalBlendShapes,
                ["skinned_renderers"] = skinnedRenderers,
                ["mesh_renderers"] = meshRenderers,
                ["node_count"] = nodeCount,
                ["meshes"] = entries,
                ["clips"] = clips,
                ["materials"] = materials,
            };
        }

        var json = MiniJson.Serialize(report);
        File.WriteAllText("unity-import-report.json", json);
        if (Directory.Exists("/tmp/sculpto-classify/cloth"))
        {
            File.WriteAllText("/tmp/sculpto-classify/cloth/unity-import-report.json", json);
        }
        Debug.Log("IMPORT_PROBE " + json);
        EditorApplication.Exit(0);
    }
}
