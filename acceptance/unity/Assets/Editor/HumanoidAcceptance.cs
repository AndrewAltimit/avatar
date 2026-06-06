// Headless acceptance check for `avatar armature fix`.
//
// Imports a candidate FBX, configures it as Humanoid with the avatar auto-created from the model
// (exactly the manual steps documented in docs/reference/armature-repair.md), and asserts Unity
// produced a valid Humanoid avatar with no hand bone assignment. This is the automated form of the
// "last mile" Unity import that the Rust tests cannot cover.
//
// Invoked in batchmode by the unity-acceptance workflow, e.g.:
//
//   Unity -batchmode -quit -projectPath acceptance/unity \
//         -executeMethod HumanoidAcceptance.Run -fbx /tmp/fixed.fbx
//
// The FBX path comes from the `-fbx <path>` CLI argument, or the AVATAR_FIXED_FBX env var as a
// fallback. Exit codes: 0 = humanoid-ready (pass), 2 = a check failed, 1 = an unexpected error.

using System;
using System.IO;
using System.Linq;
using UnityEditor;
using UnityEngine;

public static class HumanoidAcceptance
{
    public static void Run()
    {
        try
        {
            string src = ArgValue("-fbx") ?? Environment.GetEnvironmentVariable("AVATAR_FIXED_FBX");
            if (string.IsNullOrEmpty(src) || !File.Exists(src))
                Fail($"missing or unreadable FBX (pass -fbx <path> or set AVATAR_FIXED_FBX): '{src}'");

            // Copy the candidate into the project so the model importer runs on it.
            const string destDir = "Assets/Candidate";
            Directory.CreateDirectory(destDir);
            const string dest = destDir + "/fixed.fbx";
            File.Copy(src, dest, overwrite: true);
            AssetDatabase.ImportAsset(dest, ImportAssetOptions.ForceSynchronousImport);

            var importer = AssetImporter.GetAtPath(dest) as ModelImporter;
            if (importer == null)
                Fail("Unity did not import the file as a model.");

            // The two importer settings a user would set by hand: Rig -> Humanoid, and
            // "Create From This Model" so Unity auto-maps the avatar from the bone hierarchy.
            importer.animationType = ModelImporterAnimationType.Human;
            importer.avatarSetup = ModelImporterAvatarSetup.CreateFromThisModel;
            EditorUtility.SetDirty(importer);
            importer.SaveAndReimport();

            var avatar = AssetDatabase.LoadAllAssetsAtPath(dest).OfType<Avatar>().FirstOrDefault();
            if (avatar == null) Fail("no Avatar sub-asset was generated.");
            if (!avatar.isValid) Fail("the generated Avatar is not valid.");
            if (!avatar.isHuman) Fail("the generated Avatar is not Humanoid (auto-mapping failed).");

            Debug.Log("ACCEPTANCE PASS: the repaired FBX configured as a valid Humanoid avatar "
                      + "with no manual bone assignment.");
            EditorApplication.Exit(0);
        }
        catch (Exception e)
        {
            Console.Error.WriteLine("ACCEPTANCE ERROR: " + e);
            EditorApplication.Exit(1);
        }
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
