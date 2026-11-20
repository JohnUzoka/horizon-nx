Minecraft: Java Edition 26.3 (Horizon, rewritten in Rust) as Nintendo Switch homebrew. Unofficial: you need your own copy of Minecraft: Java Edition and a Switch with Atmosphère. Full instructions: [README](https://github.com/KoHaku743/horizon-nx#readme).

### Install

1. Unzip `horizon-switch-….zip` to the root of the SD card: it gives `switch/horizon/horizon.nro` and `switch/horizon/client.txt`.
2. Prepare the game files from your own Minecraft with the Horizon launcher ([playhorizon.dev](https://playhorizon.dev)) and copy the *contents* of the folder with `horizon-resources.json` into `switch/horizon/resources/` ([how](https://github.com/KoHaku743/horizon-nx#2-prepare-the-game-files-on-your-computer)).
3. Start it as an application: hold R while starting a game, then pick Horizon in the homebrew menu. Not from the Album.

On a Mac, run `dot_clean -m` on the SD card before ejecting it, or copy over FTP/MTP.

Already on 1.0.0 or later? The title screen offers new releases itself (the button left of Sign In).

### New in 1.3.0

- **Server resource packs apply in seconds.** Joining a server again with a pack you've used takes about 7 s instead of about 40 (Hypixel's). The first join with a new pack is still slow; the pack is then bundled into one file in the background (a minute or so) and used from the next join, on any server. If the server updates its pack, it is bundled again.
- **Get Mods** (Options → Mods → Get Mods): browse Horizon's mod registry in the game and install content mods and resource packs, each checked against the registry before it's installed. Mods made for a newer Horizon than this one say so instead of installing. Installed ones have a **Remove** button. Restart after installing a mod.
- **Touch on Horizon's keyboard**, which is now the default on the Switch: tap the keys, and tap chat messages while typing. The Switch's own keyboard is still in Options → Controls → Controller → On-Screen Keyboard.
- **Clickable server buttons in chat**, such as Hypixel SkyBlock's NPC choices ("[Yes!]"): open the chat and tap them.
- **Gyro aiming** (Options → Controls → Controller → Gyro Aiming): aim by turning the console or controller, always or only while attacking or using items. Off by default.

### New in 1.2.0

- **60 fps.** Built with the graphics driver release 0.1.0 used ([JohnUzoka/mesa-switch](https://github.com/JohnUzoka/mesa-switch)), which compiles the game's shaders much better: about twice the frame rate of 1.0.0 and 1.1.0.

### New in 1.1.0

- **Much faster loading.** After the first start, the game's resources load in about 15 seconds instead of 2-4 minutes. The first start after installing packs the resources folder into one file, `switch/horizon/resources.hzpak` (about 520 MB), in the background while you play; that takes about 20 minutes, and the next start uses it. The resources folder itself stays as it is.
- **Server resource packs work**, Hypixel SkyBlock included: joining with a pack takes under a minute to apply instead of disconnecting you.
- **Horizon mods (`.hmod`) work**: put them in `switch/horizon/horizon-mods/`, turn them on in Options → Mods, and restart.
- **A faster memory manager**: the console's own was about half of the game's processor time; world data now loads in under 10 seconds instead of over a minute.

### What works

- **Single player** and **multiplayer**, including online-mode servers and their resource packs.
- **Microsoft sign-in on the title screen** (Sign In, right of Realms): enter a code at microsoft.com/link on your phone. Approved by Mojang.
- **Horizon mods** (`.hmod` content mods), installed by hand or from Get Mods.
- Joy-Con (with gyro aiming), touch screen (menus and the keyboard), and HOME (with Nintendo's HOME menu; custom HOME menus such as SwitchU can freeze the console).
- In-game updates from this repository's releases.

### Known issues

- The very first start is slow (shaders compile and are kept on the SD card, and the resources are packed).
- Docked mode isn't tested yet.
- Need the SD card space: the resources pack is about 520 MB on top of the resources folder, and each server's resource pack is bundled once more (Hypixel's: about 20 MB). Deleting `resources.hzpak` makes the game use the folder again (and pack it again); `HORIZON_NX_PACK=0` in `env.txt` turns packing off.
