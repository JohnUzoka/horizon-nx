Minecraft: Java Edition 26.3 (Horizon, rewritten in Rust) as Nintendo Switch homebrew. Unofficial: you need your own copy of Minecraft: Java Edition and a Switch with Atmosphère. Full instructions: [README](https://github.com/KoHaku743/horizon-nx#readme).

### Install

1. Unzip `horizon-switch-….zip` to the root of the SD card: it gives `switch/horizon/horizon.nro` and `switch/horizon/client.txt`.
2. Prepare the game files from your own Minecraft with the Horizon launcher ([playhorizon.dev](https://playhorizon.dev)) and copy the *contents* of the folder with `horizon-resources.json` into `switch/horizon/resources/` ([how](https://github.com/KoHaku743/horizon-nx#2-prepare-the-game-files-on-your-computer)).
3. Start it as an application: hold R while starting a game, then pick Horizon in the homebrew menu. Not from the Album.

On a Mac, run `dot_clean -m` on the SD card before ejecting it, or copy over FTP/MTP.

Already on 1.0.0 or later? The title screen offers new releases itself (the button left of Sign In).

### What works

- **Single player** and **multiplayer**, including online-mode servers.
- **Microsoft sign-in on the title screen** (Sign In, right of Realms): enter a code at microsoft.com/link on your phone. Approved by Mojang.
- Joy-Con, touch screen, and HOME (with Nintendo's HOME menu; custom HOME menus such as SwitchU can freeze the console).
- In-game updates from this repository's releases.

### Known issues

- Server resource packs (for example Hypixel SkyBlock's) disconnect you; Horizon's mods aren't available on the Switch yet.
- The first start is slow (shaders compile and are kept on the SD card); the first world opens slowly too.
- Docked mode isn't tested yet.
