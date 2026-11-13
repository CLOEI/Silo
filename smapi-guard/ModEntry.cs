using System.Reflection;
using System.Text.Json;
using System.Text.Json.Serialization;
using HarmonyLib;
using Microsoft.Xna.Framework;
using Microsoft.Xna.Framework.Graphics;
using StardewModdingAPI;
using StardewModdingAPI.Events;
using StardewValley;
using StardewValley.Menus;

namespace SiloGuard;

/// <summary>
/// Deserializes <c>Mods/SiloGuard/active_profile.json</c> written by the Silo desktop manager.
/// </summary>
public sealed class ActiveProfileContract
{
    [JsonPropertyName("profile_name")]
    public string ProfileName { get; set; } = string.Empty;

    [JsonPropertyName("allowed_save_folders")]
    public List<string> AllowedSaveFolders { get; set; } = new();

    [JsonPropertyName("strict_lock")]
    public bool StrictLock { get; set; } = true;

    /// <summary>
    /// Returns <c>true</c> if the given save folder name (e.g. <c>Cendy_384920194</c>)
    /// is permitted under the active Silo profile.
    /// </summary>
    public bool IsSaveAllowed(string? saveFolderName)
    {
        if (string.IsNullOrWhiteSpace(saveFolderName))
            return true;

        // If the active profile is a standalone/unbound profile with no restricted save folders, allow all saves.
        if (this.AllowedSaveFolders.Count == 0)
            return true;

        return this.AllowedSaveFolders.Any(allowed =>
            string.Equals(allowed.Trim(), saveFolderName.Trim(), StringComparison.OrdinalIgnoreCase));
    }
}

/// <summary>
/// SMAPI companion mod entry point that enforces Silo profile save locks on <see cref="LoadGameMenu"/>.
/// </summary>
public sealed class ModEntry : Mod
{
    internal static IMonitor? LogMonitor { get; private set; }
    internal static ActiveProfileContract? ActiveContract { get; private set; }
    private static string? ContractFilePath;
    private static FieldInfo? MenuSlotsField;
    private static FieldInfo? CurrentItemIndexField;

    public override void Entry(IModHelper helper)
    {
        LogMonitor = this.Monitor;
        ContractFilePath = Path.Combine(helper.DirectoryPath, "active_profile.json");

        helper.Events.GameLoop.GameLaunched += this.OnGameLaunched;
        helper.Events.GameLoop.ReturnedToTitle += this.OnReturnedToTitle;
        helper.Events.GameLoop.SaveLoaded += this.OnSaveLoaded;
    }

    private void OnGameLaunched(object? sender, GameLaunchedEventArgs e)
    {
        ReloadActiveProfileContract();

        try
        {
            MenuSlotsField = AccessTools.Field(typeof(LoadGameMenu), "menuSlots");
            CurrentItemIndexField = AccessTools.Field(typeof(LoadGameMenu), "currentItemIndex");

            var harmony = new Harmony(this.ModManifest.UniqueID);

            // 1. Patch SaveFileSlot.Draw to render lock badge & profile overlay on incompatible saves
            harmony.Patch(
                original: AccessTools.Method(
                    typeof(LoadGameMenu.SaveFileSlot),
                    nameof(LoadGameMenu.SaveFileSlot.Draw),
                    new[] { typeof(SpriteBatch), typeof(int) }
                ),
                postfix: new HarmonyMethod(typeof(ModEntry), nameof(SaveFileSlot_Draw_Postfix))
            );

            // 2. Patch LoadGameMenu.receiveLeftClick to block clicking locked save slots
            harmony.Patch(
                original: AccessTools.Method(
                    typeof(LoadGameMenu),
                    nameof(LoadGameMenu.receiveLeftClick),
                    new[] { typeof(int), typeof(int), typeof(bool) }
                ),
                prefix: new HarmonyMethod(typeof(ModEntry), nameof(LoadGameMenu_ReceiveLeftClick_Prefix))
            );

            // 3. Patch SaveFileSlot.Activate as an additional guard before SaveGame.Load() is invoked
            harmony.Patch(
                original: AccessTools.Method(
                    typeof(LoadGameMenu.SaveFileSlot),
                    nameof(LoadGameMenu.SaveFileSlot.Activate)
                ),
                prefix: new HarmonyMethod(typeof(ModEntry), nameof(SaveFileSlot_Activate_Prefix))
            );

            this.Monitor.Log("Registered Harmony patches on LoadGameMenu and SaveFileSlot.", LogLevel.Trace);
        }
        catch (Exception ex)
        {
            this.Monitor.Log($"Failed to apply SiloGuard Harmony patches: {ex}", LogLevel.Error);
        }
    }

    private void OnReturnedToTitle(object? sender, ReturnedToTitleEventArgs e)
    {
        ReloadActiveProfileContract();
    }

    /// <summary>
    /// Fallback safety net: if a mismatched save is loaded via CLI flags or external launch arguments,
    /// immediately exit back to the title screen when <c>strict_lock</c> is enabled.
    /// </summary>
    private void OnSaveLoaded(object? sender, SaveLoadedEventArgs e)
    {
        ReloadActiveProfileContract();

        var contract = ActiveContract;
        if (contract is null || !contract.StrictLock)
            return;

        string? currentFolder = Constants.SaveFolderName;
        if (string.IsNullOrWhiteSpace(currentFolder) && !string.IsNullOrWhiteSpace(Constants.CurrentSavePath))
        {
            currentFolder = Path.GetFileName(Constants.CurrentSavePath);
        }

        if (!contract.IsSaveAllowed(currentFolder))
        {
            this.Monitor.Log(
                $"Save '{currentFolder}' is incompatible with active Silo profile '{contract.ProfileName}'. Exiting to title screen.",
                LogLevel.Error
            );
            Game1.addHUDMessage(new HUDMessage(
                $"Locked — Active Profile: {contract.ProfileName}",
                HUDMessage.error_type
            ));
            Game1.ExitToTitle();
        }
    }

    internal static void ReloadActiveProfileContract()
    {
        if (string.IsNullOrWhiteSpace(ContractFilePath) || !File.Exists(ContractFilePath))
        {
            ActiveContract = null;
            return;
        }

        try
        {
            string rawJson = File.ReadAllText(ContractFilePath);
            var contract = JsonSerializer.Deserialize<ActiveProfileContract>(rawJson);
            ActiveContract = contract;

            if (contract is not null)
            {
                LogMonitor?.Log(
                    $"Loaded active Silo profile '{contract.ProfileName}' (strict_lock={contract.StrictLock}, allowed_saves=[{string.Join(", ", contract.AllowedSaveFolders)}]).",
                    LogLevel.Info
                );
            }
        }
        catch (Exception ex)
        {
            LogMonitor?.Log($"Failed to read active_profile.json: {ex.Message}", LogLevel.Warn);
            ActiveContract = null;
        }
    }

    /// <summary>
    /// Harmony Prefix on <see cref="LoadGameMenu.receiveLeftClick"/>:
    /// Blocks left-clicks on incompatible save slots when <c>strict_lock</c> is <c>true</c>.
    /// </summary>
    public static bool LoadGameMenu_ReceiveLeftClick_Prefix(
        LoadGameMenu __instance,
        int x,
        int y
    )
    {
        var contract = ActiveContract;
        if (contract is null || !contract.StrictLock)
            return true;

        var slots = MenuSlotsField?.GetValue(__instance) as IList<LoadGameMenu.MenuSlot>;
        int currentIndex = (CurrentItemIndexField?.GetValue(__instance) as int?) ?? 0;
        if (slots is null)
            return true;

        for (int i = 0; i < __instance.slotButtons.Count; i++)
        {
            int slotIndex = currentIndex + i;
            if (slotIndex < 0 || slotIndex >= slots.Count)
                continue;

            if (!__instance.slotButtons[i].containsPoint(x, y))
                continue;

            // Ignore clicks on the delete button on the right edge if visible
            if (i < __instance.deleteButtons.Count
                && __instance.deleteButtons[i].containsPoint(x, y))
            {
                return true;
            }

            if (slots[slotIndex] is LoadGameMenu.SaveFileSlot saveSlot)
            {
                string? slotFolderName = saveSlot.Farmer?.slotName;
                if (!contract.IsSaveAllowed(slotFolderName))
                {
                    Game1.playSound("cancel");
                    LogMonitor?.Log(
                        $"Blocked click on save '{slotFolderName}' — locked by active Silo profile '{contract.ProfileName}'.",
                        LogLevel.Warn
                    );
                    return false;
                }
            }
        }

        return true;
    }

    /// <summary>
    /// Harmony Prefix on <see cref="LoadGameMenu.SaveFileSlot.Activate"/>:
    /// Blocks <c>SaveGame.Load()</c> when <c>strict_lock</c> is <c>true</c> and the save slot is not allowed.
    /// </summary>
    public static bool SaveFileSlot_Activate_Prefix(LoadGameMenu.SaveFileSlot __instance)
    {
        var contract = ActiveContract;
        if (contract is null)
            return true;

        string? slotFolderName = __instance.Farmer?.slotName;
        if (contract.IsSaveAllowed(slotFolderName))
            return true;

        if (contract.StrictLock)
        {
            Game1.playSound("cancel");
            LogMonitor?.Log(
                $"Blocked loading save '{slotFolderName}' — locked by active Silo profile '{contract.ProfileName}'.",
                LogLevel.Warn
            );
            return false;
        }

        return true;
    }

    /// <summary>
    /// Harmony Postfix on <see cref="LoadGameMenu.SaveFileSlot.Draw"/>:
    /// Renders a lock badge and <c>"Locked — Active Profile: [Profile Name]"</c> overlay on incompatible saves.
    /// </summary>
    public static void SaveFileSlot_Draw_Postfix(
        LoadGameMenu.SaveFileSlot __instance,
        SpriteBatch b,
        int i
    )
    {
        var contract = ActiveContract;
        if (contract is null)
            return;

        string? slotFolderName = __instance.Farmer?.slotName;
        if (contract.IsSaveAllowed(slotFolderName))
            return;

        if (Game1.activeClickableMenu is not LoadGameMenu loadMenu)
            return;

        if (i < 0 || i >= loadMenu.slotButtons.Count)
            return;

        Rectangle slotBounds = loadMenu.slotButtons[i].bounds;

        // Draw a translucent dark tint over the incompatible save slot
        b.Draw(
            Game1.staminaRect,
            new Rectangle(slotBounds.X + 8, slotBounds.Y + 4, slotBounds.Width - 16, slotBounds.Height - 8),
            new Color(20, 12, 16, 175)
        );

        // Draw banner container on the center of the save slot
        string bannerText = contract.StrictLock
            ? $"Locked — Active Profile: {contract.ProfileName}"
            : $"Profile Mismatch: {contract.ProfileName}";

        Vector2 textSize = Game1.dialogueFont.MeasureString(bannerText);
        int paddingX = 20;
        int paddingY = 12;
        int badgeWidth = (int)textSize.X + paddingX * 2 + 28;
        int badgeHeight = (int)textSize.Y + paddingY * 2;

        int badgeX = slotBounds.X + (slotBounds.Width - badgeWidth) / 2;
        int badgeY = slotBounds.Y + (slotBounds.Height - badgeHeight) / 2;

        IClickableMenu.drawTextureBox(
            b,
            Game1.mouseCursors,
            new Rectangle(384, 396, 15, 15),
            badgeX,
            badgeY,
            badgeWidth,
            badgeHeight,
            contract.StrictLock ? new Color(180, 50, 50) : new Color(210, 140, 40),
            4f,
            drawShadow: true
        );

        // Draw padlock / warning indicator icon using vanilla cursor sprites
        b.Draw(
            Game1.mouseCursors,
            new Vector2(badgeX + paddingX, badgeY + (badgeHeight - 28) / 2),
            new Rectangle(268, 470, 16, 16),
            Color.White,
            0f,
            Vector2.Zero,
            1.75f,
            SpriteEffects.None,
            0.9f
        );

        // Draw the lock label text
        Utility.drawTextWithShadow(
            b,
            bannerText,
            Game1.dialogueFont,
            new Vector2(badgeX + paddingX + 34, badgeY + paddingY),
            Color.White,
            1f,
            -1f,
            -1,
            -1,
            0.75f
        );
    }
}
