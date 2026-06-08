// Headless acceptance check for `avatar anim-gen` (the M4 asset generator).
//
// The Rust tests only round-trip generated `.anim`/`.controller` YAML through our *own* reader
// (avatar-unity-yaml / avatar-unity-asset). That proves we can read what we wrote, not that Unity
// accepts it. This harness closes that gap: it imports CLI-generated assets in a real editor and
// asserts Unity parsed each one into the expected object type with **no import errors logged**
// (a malformed fileID/field shows up as a console error even when an object is still produced).
//
// Invoked in batchmode by the unity-acceptance workflow, e.g.:
//
//   Unity -batchmode -quit -projectPath acceptance/unity \
//         -executeMethod GeneratedAssetAcceptance.Run -assets /tmp/gen
//
// The assets directory comes from `-assets <dir>` (or the AVATAR_GEN_ASSETS env var). Every `.anim`
// and `.controller` directly under it is imported and checked. A `.controller` may reference clip
// GUIDs not present in this project; that surfaces as a *missing motion* (a warning), not an error,
// so it does not fail the structural check. Exit codes: 0 = pass, 2 = a check failed, 1 = error.

using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using UnityEditor;
using UnityEditor.Animations;
using UnityEngine;

public static class GeneratedAssetAcceptance
{
    static readonly List<string> ImportErrors = new List<string>();

    public static void Run()
    {
        try
        {
            string dir = ArgValue("-assets") ?? Environment.GetEnvironmentVariable("AVATAR_GEN_ASSETS");
            if (string.IsNullOrEmpty(dir) || !Directory.Exists(dir))
                Fail($"missing or unreadable assets dir (pass -assets <dir> or set AVATAR_GEN_ASSETS): '{dir}'");

            var assets = Directory.GetFiles(dir)
                .Where(f => f.EndsWith(".anim") || f.EndsWith(".controller"))
                .OrderBy(f => f)
                .ToList();
            if (assets.Count == 0)
                Fail($"no .anim or .controller files found under '{dir}'");

            const string destDir = "Assets/Generated";
            Directory.CreateDirectory(destDir);

            // Capture any error Unity logs while importing — a YAML/field problem logs an error here.
            Application.logMessageReceived += OnLog;
            var imported = new List<string>();
            try
            {
                foreach (var src in assets)
                {
                    string dest = destDir + "/" + Path.GetFileName(src);
                    File.Copy(src, dest, overwrite: true);
                    AssetDatabase.ImportAsset(dest, ImportAssetOptions.ForceSynchronousImport);
                    imported.Add(dest);
                }
            }
            finally
            {
                Application.logMessageReceived -= OnLog;
            }

            if (ImportErrors.Count > 0)
                Fail("Unity logged import error(s):\n  " + string.Join("\n  ", ImportErrors));

            foreach (var dest in imported)
            {
                if (dest.EndsWith(".anim")) CheckClip(dest);
                else if (dest.EndsWith(".controller")) CheckController(dest);
            }

            Debug.Log($"ACCEPTANCE PASS: Unity imported {imported.Count} generated asset(s) "
                      + "with no errors and the expected object types.");
            EditorApplication.Exit(0);
        }
        catch (Exception e)
        {
            Console.Error.WriteLine("ACCEPTANCE ERROR: " + e);
            EditorApplication.Exit(1);
        }
    }

    static void CheckClip(string dest)
    {
        var clip = AssetDatabase.LoadAssetAtPath<AnimationClip>(dest);
        if (clip == null) Fail($"{dest} did not import as an AnimationClip.");
        // A generated FX clip carries at least one curve; an empty clip means our curves were dropped.
        var bindings = AnimationUtility.GetCurveBindings(clip)
            .Concat(AnimationUtility.GetObjectReferenceCurveBindings(clip));
        if (!bindings.Any())
            Fail($"{dest} imported as an AnimationClip with no curves (curves were lost).");
        Debug.Log($"  ok: {dest} → AnimationClip '{clip.name}'");
    }

    static void CheckController(string dest)
    {
        var ac = AssetDatabase.LoadAssetAtPath<AnimatorController>(dest);
        if (ac == null) Fail($"{dest} did not import as an AnimatorController.");
        if (ac.parameters == null || ac.parameters.Length == 0)
            Fail($"{dest} imported with no animator parameters.");
        if (ac.layers == null || ac.layers.Length == 0)
            Fail($"{dest} imported with no animator layers.");
        // The single layer should carry a state whose motion is a BlendTree (what we emit).
        bool hasBlendTree = ac.layers
            .Where(l => l.stateMachine != null)
            .SelectMany(l => l.stateMachine.states)
            .Any(s => s.state != null && s.state.motion is BlendTree);
        if (!hasBlendTree)
            Fail($"{dest} has no state whose motion is a BlendTree.");
        Debug.Log($"  ok: {dest} → AnimatorController '{ac.name}' "
                  + $"({ac.parameters.Length} param(s), {ac.layers.Length} layer(s), blend tree present)");
    }

    static void OnLog(string condition, string stackTrace, LogType type)
    {
        if (type == LogType.Error || type == LogType.Exception)
            ImportErrors.Add(condition);
    }

    static string ArgValue(string flag)
    {
        var args = Environment.GetCommandLineArgs();
        for (int i = 0; i < args.Length - 1; i++)
            if (args[i] == flag)
                return args[i + 1];
        return null;
    }

    static void Fail(string msg)
    {
        Console.Error.WriteLine("ACCEPTANCE FAIL: " + msg);
        EditorApplication.Exit(2);
    }
}
