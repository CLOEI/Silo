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
    private static DateTime LastContractCheckUtc = DateTime.MinValue;
    private static DateTime LastContractWriteUtc = DateTime.MinValue;
    private static FieldInfo? MenuSlotsField;
    private static FieldInfo? CurrentItemIndexField;
    private static FieldInfo? SlotParentMenuField;
    private static string? WarnConfirmedSaveFolder;
    private static bool PendingInGameMismatchWarning;

    public override void Entry(IModHelper helper)
    {
        LogMonitor = this.Monitor;
        ContractFilePath = Path.Combine(helper.DirectoryPath, "active_profile.json");

        helper.Events.GameLoop.GameLaunched += this.OnGameLaunched;
        helper.Events.GameLoop.ReturnedToTitle += this.OnReturnedToTitle;
        helper.Events.GameLoop.SaveLoaded += this.OnSaveLoaded;
        helper.Events.GameLoop.DayStarted += this.OnDayStarted;
        helper.Events.Display.MenuChanged += this.OnMenuChanged;
    }

    private void OnMenuChanged(object? sender, MenuChangedEventArgs e)
    {
        WarnConfirmedSaveFolder = null;
        ReloadActiveProfileContract(force: true);
    }

    private void OnGameLaunched(object? sender, GameLaunchedEventArgs e)
    {
        ReloadActiveProfileContract(force: true);

        try
        {
            MenuSlotsField = AccessTools.Field(typeof(LoadGameMenu), "menuSlots");
            CurrentItemIndexField = AccessTools.Field(typeof(LoadGameMenu), "currentItemIndex");
            SlotParentMenuField = AccessTools.Field(typeof(LoadGameMenu.MenuSlot), "menu");

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
        WarnConfirmedSaveFolder = null;
        PendingInGameMismatchWarning = false;
        ReloadActiveProfileContract(force: true);
    }

    private void OnDayStarted(object? sender, DayStartedEventArgs e)
    {
        if (!PendingInGameMismatchWarning)
            return;

        PendingInGameMismatchWarning = false;
        var contract = ActiveContract;
        if (contract is not null && !contract.StrictLock)
        {
            Game1.addHUDMessage(new HUDMessage(
                $"SiloGuard Warning - Active Profile: {contract.ProfileName}",
                HUDMessage.error_type
            ));
        }
    }

    /// <summary>
    /// Fallback safety net: if a mismatched save is loaded via CLI flags or external launch arguments,
    /// immediately exit back to the title screen when <c>strict_lock</c> is enabled.
    /// </summary>
    private void OnSaveLoaded(object? sender, SaveLoadedEventArgs e)
    {
        ReloadActiveProfileContract(force: true);

        var contract = ActiveContract;
        if (contract is null)
            return;

        string? currentFolder = Constants.SaveFolderName;
        if (string.IsNullOrWhiteSpace(currentFolder) && !string.IsNullOrWhiteSpace(Constants.CurrentSavePath))
        {
            currentFolder = Path.GetFileName(Constants.CurrentSavePath);
        }

        if (!contract.IsSaveAllowed(currentFolder))
        {
            if (contract.StrictLock)
            {
                this.Monitor.Log(
                    $"Save '{currentFolder}' is incompatible with active Silo profile '{contract.ProfileName}'. Exiting to title screen.",
                    LogLevel.Error
                );
                Game1.addHUDMessage(new HUDMessage(
                    $"Locked - Active Profile: {contract.ProfileName}",
                    HUDMessage.error_type
                ));
                Game1.ExitToTitle();
            }
            else
            {
                this.Monitor.Log(
                    $"Warning: Loaded save '{currentFolder}' under mismatched Silo profile '{contract.ProfileName}' (Warn Only mode).",
                    LogLevel.Warn
                );
                PendingInGameMismatchWarning = true;
                Game1.addHUDMessage(new HUDMessage(
                    $"SiloGuard Warning - Active Profile: {contract.ProfileName}",
                    HUDMessage.error_type
                ));
            }
        }
    }

    internal static void ReloadActiveProfileContract(bool force = false)
    {
        DateTime now = DateTime.UtcNow;
        if (!force && (now - LastContractCheckUtc).TotalMilliseconds < 500)
            return;
        LastContractCheckUtc = now;

        if (string.IsNullOrWhiteSpace(ContractFilePath) || !File.Exists(ContractFilePath))
        {
            ActiveContract = null;
            return;
        }

        try
        {
            DateTime writeUtc = File.GetLastWriteTimeUtc(ContractFilePath);
            if (!force && ActiveContract is not null && writeUtc == LastContractWriteUtc)
                return;

            LastContractWriteUtc = writeUtc;
            string rawJson = File.ReadAllText(ContractFilePath);
            var contract = JsonSerializer.Deserialize<ActiveProfileContract>(rawJson);
            ActiveContract = contract;
            WarnConfirmedSaveFolder = null;

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

    private static LoadGameMenu? ResolveLoadGameMenu(LoadGameMenu.SaveFileSlot? slot = null)
    {
        if (Game1.activeClickableMenu is LoadGameMenu directMenu)
            return directMenu;

        if (TitleMenu.subMenu is LoadGameMenu titleSubMenu)
            return titleSubMenu;

        if (slot is not null && SlotParentMenuField?.GetValue(slot) is LoadGameMenu slotMenu)
            return slotMenu;

        return null;
    }

    /// <summary>
    /// Harmony Prefix on <see cref="LoadGameMenu.receiveLeftClick"/>:
    /// Blocks left-clicks on incompatible save slots when <c>strict_lock</c> is <c>true</c>,
    /// and requires a confirmation second-click when <c>strict_lock</c> is <c>false</c> (Warn Only mode).
    /// </summary>
    public static bool LoadGameMenu_ReceiveLeftClick_Prefix(
        LoadGameMenu __instance,
        int x,
        int y
    )
    {
        ReloadActiveProfileContract();
        var contract = ActiveContract;
        if (contract is null)
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
                    if (contract.StrictLock)
                    {
                        Game1.playSound("cancel");
                        LogMonitor?.Log(
                            $"Blocked click on save '{slotFolderName}' - locked by active Silo profile '{contract.ProfileName}'.",
                            LogLevel.Warn
                        );
                        return false;
                    }

                    // Warn Only mode: first click arms confirmation & shows clear warning; second click loads
                    if (!string.Equals(WarnConfirmedSaveFolder, slotFolderName, StringComparison.OrdinalIgnoreCase))
                    {
                        WarnConfirmedSaveFolder = slotFolderName;
                        Game1.playSound("drumkit6");
                        LogMonitor?.Log(
                            $"Warned on save '{slotFolderName}' - mismatched with active Silo profile '{contract.ProfileName}'. Click again to load anyway.",
                            LogLevel.Warn
                        );
                        return false;
                    }

                    WarnConfirmedSaveFolder = null;
                    return true;
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
        ReloadActiveProfileContract();
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
                $"Blocked loading save '{slotFolderName}' - locked by active Silo profile '{contract.ProfileName}'.",
                LogLevel.Warn
            );
            return false;
        }

        LogMonitor?.Log(
            $"Warning: Loading save '{slotFolderName}' with mismatched active Silo profile '{contract.ProfileName}' (Warn Only mode).",
            LogLevel.Warn
        );
        return true;
    }

    /// <summary>
    /// Harmony Postfix on <see cref="LoadGameMenu.SaveFileSlot.Draw"/>:
    /// Renders a lock badge (<c>StrictLock</c>) or warning badge (<c>Warn Only</c>) on incompatible saves.
    /// </summary>
    public static void SaveFileSlot_Draw_Postfix(
        LoadGameMenu.SaveFileSlot __instance,
        SpriteBatch b,
        int i
    )
    {
        ReloadActiveProfileContract();
        var contract = ActiveContract;
        if (contract is null)
            return;

        string? slotFolderName = __instance.Farmer?.slotName;
        if (contract.IsSaveAllowed(slotFolderName))
            return;

        LoadGameMenu? loadMenu = ResolveLoadGameMenu(__instance);
        if (loadMenu is null)
            return;

        if (i < 0 || i >= loadMenu.slotButtons.Count)
            return;

        Rectangle slotBounds = loadMenu.slotButtons[i].bounds;
        bool isArmedWarn = !contract.StrictLock
            && !string.IsNullOrWhiteSpace(slotFolderName)
            && string.Equals(WarnConfirmedSaveFolder, slotFolderName, StringComparison.OrdinalIgnoreCase);

        // Draw a translucent tint over the incompatible save slot
        b.Draw(
            Game1.staminaRect,
            new Rectangle(slotBounds.X + 8, slotBounds.Y + 4, slotBounds.Width - 16, slotBounds.Height - 8),
            contract.StrictLock
                ? new Color(20, 12, 16, 175)
                : new Color(36, 24, 8, 150)
        );

        // Draw banner container on the center of the save slot
        string bannerText = contract.StrictLock
            ? $"Locked - Active Profile: {contract.ProfileName}"
            : isArmedWarn
                ? $"Click Again to Load Anyway ({contract.ProfileName})"
                : $"Warning - Profile Mismatch: {contract.ProfileName}";

        SpriteFont font = Game1.dialogueFont;
        Vector2 textSize = font.MeasureString(bannerText);
        int paddingX = 20;
        int paddingY = 12;
        int badgeWidth = Math.Min(slotBounds.Width - 32, (int)textSize.X + paddingX * 2 + 28);
        int badgeHeight = (int)textSize.Y + paddingY * 2;

        int badgeX = slotBounds.X + (slotBounds.Width - badgeWidth) / 2;
        int badgeY = slotBounds.Y + (slotBounds.Height - badgeHeight) / 2;

        Color badgeColor = contract.StrictLock
            ? new Color(180, 50, 50)
            : isArmedWarn
                ? new Color(230, 95, 25)
                : new Color(210, 140, 40);

        IClickableMenu.drawTextureBox(
            b,
            Game1.mouseCursors,
            new Rectangle(384, 396, 15, 15),
            badgeX,
            badgeY,
            badgeWidth,
            badgeHeight,
            badgeColor,
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

        // Draw the lock/warning label text
        Utility.drawTextWithShadow(
            b,
            bannerText,
            font,
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
