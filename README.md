# Silo

**Silo** is a fast, save-centric Stardew Valley mod and profile manager built with Rust (egui + [fastframe](https://github.com/crmne/fastframe)) and a companion SMAPI mod (`SiloGuard`).

Instead of duplicating mod folders or risking loading a heavily modded save with the wrong mod list, Silo stores mods once in a central vault, links them per save/profile using zero-copy OS junctions/symlinks, and guards your save slots directly inside Stardew Valley.

---

## Features

- **Save-Centric Profiles**: Automatically scans your Stardew Valley `Saves` folder, parses farmer/farm metadata (`SaveGameInfo`), and pairs each save with its own mod profile.
- **Standalone & Cloned Profiles**: Clone any profile in one click or create standalone profiles for testing new modpacks.
- **Zero-Copy Mod Vault**: Stores unpacked mods once in `~/.silo/vault/` and mounts enabled mods into `<Stardew Valley>/Mods/` using NTFS Directory Junctions (Windows) or POSIX symlinks (macOS / Linux) with automatic copy fallback on exFAT/SD cards.
- **SMAPI Dependency Resolver**: Parses `manifest.json` files, flags missing/disabled/outdated dependencies, and offers 1-click fixes when required mods are in your vault.
- **Nexus Mods Integration**:
  - Browse curated and live Nexus Mods feeds with category filters and search.
  - **Mod Manager Download** support.
  - Automatically checks installed mods for updates on startup and profile selection.
- **`SiloGuard` In-Game Save Protection**:
  - Auto-deployed SMAPI companion mod (`Mods/SiloGuard/SiloGuard.dll`) synced with your active Silo profile.
  - **Strict Lock**: Locks incompatible saves on Stardew Valley's Load Game screen so you never accidentally corrupt a save.
  - **Warn Only**: Displays an amber mismatch warning on save slots and asks for confirmation before loading.
- **Native Desktop UI**: Clean single-window interface with Light/Dark mode.

---

## Project Structure

```text
Silo/
├── crates/
│   ├── silo-core/      # SQLite database, mod vault, save XML parser, SMAPI manifest & Nexus client
│   ├── silo-platform/  # OS path detection, NTFS junctions / symlinks, SiloGuard deployment, nxm:// IPC
│   └── silo-ui/        # egui + fastframe desktop application
└── smapi-guard/        # C# (.NET 6.0) SiloGuard SMAPI companion mod
```

---

## Getting Started

### Prerequisites
- **Rust** (`1.98+` / Edition 2024)
- **Stardew Valley** with **SMAPI** installed
- *(Optional, only if modifying `SiloGuard`)* **.NET 6.0 SDK**

### Run Silo

```bash
cargo run -p silo-ui --release
```

### Rebuild `SiloGuard` (Optional)
A precompiled `SiloGuard.dll` is bundled in `smapi-guard/bin/Release/net6.0/SiloGuard.dll` and embedded into Silo at compile time. To rebuild the C# mod after editing `smapi-guard/ModEntry.cs`:

```bash
dotnet build smapi-guard/SiloGuard.csproj -c Release
```

---

## How It Works

1. **Select a Save or Profile** in the left sidebar.
2. **Enable / Disable Mods** for that profile, or install new mods from **Discover Mods** or **Settings → Import Archive / Folder**.
3. Click **Mount Mods** (or **Play SMAPI**) in the top bar to link the profile's mods into `<Stardew Valley>/Mods/` and update `SiloGuard`.
4. Configure your **Nexus Personal API Key** and global **SiloGuard Save Protection** mode (`Strict Lock ON` vs `Warn Only`) anytime in **Settings**.

---

## AI Disclosure

AI tools were used to accelerate parts of the development process. However, all product decisions, architecture, code reviews, UX direction, and many parts of the implementation are done by a human.
