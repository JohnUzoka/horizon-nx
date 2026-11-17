Minecraft: Java Edition 26.3 (Horizon, rewritten in Rust) as Nintendo Switch homebrew. Unofficial: you need your own copy of Minecraft: Java Edition and a Switch with Atmosphère. Full instructions: [README](https://github.com/KoHaku743/horizon-nx#readme).

### Install

1. Unzip this file to the root of the SD card: it gives `switch/horizon/horizon.nro` and `switch/horizon/client.txt`.
2. Prepare the game files from your own Minecraft with the Horizon launcher ([playhorizon.dev](https://playhorizon.dev)) and copy the *contents* of the folder with `horizon-resources.json` into `switch/horizon/resources/` ([how](https://github.com/KoHaku743/horizon-nx#2-prepare-the-game-files-on-your-computer)).
3. Start it as an application: hold R while starting a game, then pick Horizon in the homebrew menu. Not from the Album.

On a Mac, run `dot_clean -m` on the SD card before ejecting it, or copy over FTP/MTP.

### In this build

- Built from the source of the original port's 0.1.0, with this fork's public build setup.
- **GPU hang fixed:** the graphics driver's `plat` and `ofold` shader optimizations hung the GPU about 10 s after terrain appeared; both are off by default.
- **HOME works** with Nintendo's HOME menu (custom HOME menus such as SwitchU can freeze the console).
- **Microsoft sign-in on the title screen** (Sign In, right of Realms). Until Mojang approves the app it stops at the Minecraft step.

### Known issues

- The first start is slow (shaders compile and are kept on the SD card); the first world opens slowly too.
- Docked mode isn't tested yet.
