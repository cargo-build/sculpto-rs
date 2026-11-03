using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using UnityEditor;
using UnityEngine;

/// <summary>Headless glTF import probe: reports mesh, morph and skinning stats.</summary>
public static class ImportProbe
{
    public static void Run()
    {
        var report = new Dictionary<string, object>();
        AssetDatabase.Refresh();
        var forced = AssetDatabase.LoadMainAssetAtPath("Assets/Garments/character.glb");
        if (forced == null || AssetDatabase.LoadAllAssetsAtPath("Assets/Garments/character.glb").Length < 2)
        {
            AssetDatabase.ImportAsset("Assets/Garments/character.glb", ImportAssetOptions.ForceUpdate);
        }
        foreach (var path in new[] { "Assets/Garments/character.glb", "Assets/Garments/C1-cloth-idle.glb" })
        {
            var assets = AssetDatabase.LoadAllAssetsAtPath(path);
            var meshes = assets.OfType<Mesh>().ToList();
            var entries = new List<Dictionary<string, object>>();
            foreach (var mesh in meshes)
            {
                entries.Add(new Dictionary<string, object>
                {
                    ["name"] = mesh.name,
                    ["vertices"] = mesh.vertexCount,
                    ["blendShapes"] = mesh.blendShapeCount,
                    ["subMeshes"] = mesh.subMeshCount,
                    ["bounds"] = mesh.bounds.size.ToString(),
                    ["bindposes"] = mesh.bindposeCount,
                });
            }
            report[path] = new Dictionary<string, object>
            {
                ["meshes"] = entries,
                ["clips"] = assets.OfType<AnimationClip>().Select(c => c.name + ":" + c.length.ToString("0.00")).ToList(),
                ["renderers"] = assets.OfType<SkinnedMeshRenderer>().Count(),
                ["materials"] = assets.OfType<Material>().Select(m => m.name).ToList(),
            };
        }
        var json = JsonUtility.ToJson(new Wrapper { text = MiniJson.Serialize(report) }, true);
        File.WriteAllText("/tmp/sculpto-classify/cloth/unity-import-report.json", MiniJson.Serialize(report));
        Debug.Log("IMPORT_PROBE " + MiniJson.Serialize(report));
        EditorApplication.Exit(0);
    }

    [Serializable]
    private class Wrapper { public string text; }
}
