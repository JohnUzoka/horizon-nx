use super::*;

/// A file system in memory: folders, and files with their bytes (manifests among them).
#[derive(Default)]
struct Memory {
    dirs: std::collections::BTreeSet<String>,
    files: BTreeMap<String, Vec<u8>>,
    released: usize,
}

impl Memory {
    fn with_dirs(dirs: &[&str]) -> Self {
        let mut memory = Memory::default();
        memory.dirs.insert("sdmc:/".into());
        for dir in dirs {
            memory.dirs.insert((*dir).into());
        }
        memory
    }
    /// A real rename of `old` (and everything under it) to `new`.
    fn rename(&mut self, old: &str, new: &str) {
        let moved = |path: &String| {
            (path == old || path.starts_with(&join(old, "")))
                .then(|| format!("{new}{}", &path[old.len()..]))
        };
        self.dirs = std::mem::take(&mut self.dirs)
            .into_iter()
            .map(|d| moved(&d).unwrap_or(d))
            .collect();
        self.files = std::mem::take(&mut self.files)
            .into_iter()
            .map(|(f, b)| (moved(&f).unwrap_or(f), b))
            .collect();
    }
}

impl Store for Memory {
    fn read(&mut self, dir: &str) -> Option<Vec<u8>> {
        self.files.get(&join(dir, MANIFEST)).cloned()
    }
    fn append(&mut self, dir: &str, text: &str) -> Result<(), i32> {
        self.files
            .entry(join(dir, MANIFEST))
            .or_default()
            .extend_from_slice(text.as_bytes());
        Ok(())
    }
    fn remove(&mut self, dir: &str) -> Result<(), i32> {
        self.files.remove(&join(dir, MANIFEST));
        Ok(())
    }
    fn kind(&mut self, path: &str) -> Option<bool> {
        if self.dirs.contains(path) {
            Some(true)
        } else {
            self.files.contains_key(path).then_some(false)
        }
    }
    fn release(&mut self) {
        self.released += 1;
    }
}

fn p(path: &str) -> Parts {
    normalize(path, "sdmc:/switch/horizon")
}

fn resolved(links: &Links<Memory>, path: &str) -> String {
    render(&links.resolve(&p(path), true).unwrap())
}

fn manifest(links: &Links<Memory>, dir: &str) -> Option<String> {
    let store = lock(&links.store);
    store
        .files
        .get(&join(dir, MANIFEST))
        .map(|b| String::from_utf8(b.clone()).unwrap())
}

/// The overlay the pack builder makes: `out` holds links to vanilla's folders and a pack's file.
fn overlay() -> Links<Memory> {
    let mut memory = Memory::with_dirs(&[
        "sdmc:/switch",
        "sdmc:/switch/horizon",
        "sdmc:/switch/horizon/res",
        "sdmc:/switch/horizon/res/assets",
        "sdmc:/switch/horizon/res/assets/minecraft",
        "sdmc:/switch/horizon/res/assets/minecraft/textures",
        "sdmc:/switch/horizon/pack",
        "sdmc:/switch/horizon/out",
        "sdmc:/switch/horizon/out/assets",
    ]);
    memory.files.insert(
        "sdmc:/switch/horizon/res/assets/minecraft/textures/stone.png".into(),
        b"stone".to_vec(),
    );
    memory.files.insert(
        "sdmc:/switch/horizon/pack/dirt.png".into(),
        b"dirt".to_vec(),
    );
    let links = Links::new(memory);
    links
        .symlink(
            "sdmc:/switch/horizon/res/assets/minecraft",
            &p("out/assets/minecraft"),
        )
        .unwrap();
    // Relative, as `mod_content.rs` passes them: from the link's own folder.
    links
        .symlink("../../pack/dirt.png", &p("out/assets/dirt.png"))
        .unwrap();
    links
}

#[test]
fn normalize_handles_devices_cwd_dots_and_double_devices() {
    assert_eq!(p("a/./b//c/../d"), ["switch", "horizon", "a", "b", "d"]);
    assert_eq!(p("/x"), ["x"]);
    assert_eq!(p("sdmc:/x/y"), ["x", "y"]);
    // std::path::absolute's join onto a path with a device.
    assert_eq!(p("sdmc:/switch/horizon/sdmc:/x"), ["x"]);
    assert_eq!(render(&p("sdmc:/")), "sdmc:/");
    assert_eq!(render(&p("sdmc:/a/b")), "sdmc:/a/b");
}

#[test]
fn paths_go_through_links_to_folders_and_files() {
    let links = overlay();
    assert_eq!(
        resolved(&links, "out/assets/minecraft/textures/stone.png"),
        "sdmc:/switch/horizon/res/assets/minecraft/textures/stone.png"
    );
    assert_eq!(
        resolved(&links, "out/assets/dirt.png"),
        "sdmc:/switch/horizon/pack/dirt.png"
    );
    // Not followed: the link itself.
    let (at, target) = links.entry(&p("out/assets/dirt.png")).unwrap();
    assert_eq!(render(&at), "sdmc:/switch/horizon/out/assets/dirt.png");
    assert_eq!(
        target.as_deref(),
        Some("sdmc:/switch/horizon/pack/dirt.png")
    );
    assert_eq!(
        links.readlink(&p("out/assets/minecraft")).unwrap(),
        "sdmc:/switch/horizon/res/assets/minecraft"
    );
    assert_eq!(links.readlink(&p("out/assets")), Err(EINVAL));
    // A path with no link on it stays as it is.
    assert_eq!(
        resolved(&links, "pack/dirt.png"),
        "sdmc:/switch/horizon/pack/dirt.png"
    );
    let mut names = links.names(&p("out/assets"));
    names.sort();
    assert_eq!(names, ["dirt.png", "minecraft"]);
}

#[test]
fn links_through_links_resolve_and_loops_stop() {
    let links = overlay();
    // The mod content root links into the pack overlay, which links into vanilla.
    lock(&links.store)
        .dirs
        .insert("sdmc:/switch/horizon/mods".into());
    links
        .symlink("sdmc:/switch/horizon/out/assets", &p("mods/assets"))
        .unwrap();
    assert_eq!(
        resolved(&links, "mods/assets/minecraft/textures/stone.png"),
        "sdmc:/switch/horizon/res/assets/minecraft/textures/stone.png"
    );
    links.symlink("b", &p("pack/a")).unwrap();
    links.symlink("a", &p("pack/b")).unwrap();
    assert_eq!(links.resolve(&p("pack/a"), true), Err(ELOOP));
    assert!(links.resolve(&p("pack/a"), false).is_ok());
}

#[test]
fn links_survive_a_restart_through_their_manifests() {
    let links = overlay();
    let memory = links.store.into_inner().unwrap();
    let again = Links::new(memory);
    assert_eq!(
        resolved(&again, "out/assets/minecraft/textures/stone.png"),
        "sdmc:/switch/horizon/res/assets/minecraft/textures/stone.png"
    );
}

#[test]
fn a_torn_last_line_is_ignored() {
    let links = overlay();
    let mut memory = links.store.into_inner().unwrap();
    memory
        .files
        .get_mut("sdmc:/switch/horizon/out/assets/.hzlinks")
        .unwrap()
        .extend_from_slice(b"-dirt.png");
    let again = Links::new(memory);
    assert_eq!(
        resolved(&again, "out/assets/dirt.png"),
        "sdmc:/switch/horizon/pack/dirt.png"
    );
}

#[test]
fn unlink_removes_links_and_the_last_one_takes_the_manifest() {
    let links = overlay();
    assert_eq!(links.unlink(&p("out/assets/dirt.png")), Ok(Then::Done));
    assert_eq!(
        manifest(&links, "sdmc:/switch/horizon/out/assets").unwrap(),
        "+minecraft\tsdmc:/switch/horizon/res/assets/minecraft\n+dirt.png\tsdmc:/switch/horizon/pack/dirt.png\n-dirt.png\n"
    );
    assert_eq!(links.entry(&p("out/assets/dirt.png")).unwrap().1, None);
    // Unlinking a link to a folder removes the link, not the folder's files.
    assert_eq!(links.unlink(&p("out/assets/minecraft")), Ok(Then::Done));
    assert_eq!(manifest(&links, "sdmc:/switch/horizon/out/assets"), None);
    assert!(
        lock(&links.store)
            .files
            .contains_key("sdmc:/switch/horizon/res/assets/minecraft/textures/stone.png")
    );
    // A real file is left to the file system.
    assert_eq!(
        links.unlink(&p("pack/dirt.png")),
        Ok(Then::Real("sdmc:/switch/horizon/pack/dirt.png".into()))
    );
}

#[test]
fn rmdir_needs_the_links_gone_first() {
    let links = overlay();
    assert_eq!(links.rmdir(&p("out/assets")), Err(ENOTEMPTY));
    assert_eq!(links.rmdir(&p("out/assets/minecraft")), Err(ENOTDIR));
    links.unlink(&p("out/assets/dirt.png")).unwrap();
    links.unlink(&p("out/assets/minecraft")).unwrap();
    assert_eq!(
        links.rmdir(&p("out/assets")),
        Ok("sdmc:/switch/horizon/out/assets".into())
    );
}

#[test]
fn symlink_refuses_taken_names_and_missing_folders() {
    let links = overlay();
    assert_eq!(links.symlink("x", &p("out/assets/dirt.png")), Err(EEXIST));
    assert_eq!(links.symlink("x", &p("pack/dirt.png")), Err(EEXIST));
    assert_eq!(links.symlink("x", &p("nowhere/link")), Err(ENOENT));
    assert_eq!(links.symlink("x", &p("pack/dirt.png/inside")), Err(ENOTDIR));
    assert_eq!(links.symlink("x", &p("out/.hzlinks")), Err(EINVAL));
    assert_eq!(links.mkdir(&p("out/assets/minecraft")), Err(EEXIST));
    assert_eq!(
        links.mkdir(&p("out/new")),
        Ok("sdmc:/switch/horizon/out/new".into())
    );
}

#[test]
fn renaming_a_link_moves_it_and_renaming_a_folder_moves_its_links() {
    let links = overlay();
    assert_eq!(
        links.rename(&p("out/assets/dirt.png"), &p("out/dirt2.png")),
        Ok(Then::Done)
    );
    assert_eq!(
        resolved(&links, "out/dirt2.png"),
        "sdmc:/switch/horizon/pack/dirt.png"
    );
    assert_eq!(links.entry(&p("out/assets/dirt.png")).unwrap().1, None);
    // A real folder: the file system moves it (manifests inside) and the links follow.
    let Then::Rename(old, new) = links.rename(&p("out"), &p("out-old")).unwrap() else {
        panic!("a real folder is renamed by the file system");
    };
    assert!(lock(&links.store).released > 0);
    lock(&links.store).rename(&old, &new);
    links.renamed(&old, &new);
    assert_eq!(
        resolved(&links, "out-old/assets/minecraft/textures/stone.png"),
        "sdmc:/switch/horizon/res/assets/minecraft/textures/stone.png"
    );
    assert_eq!(links.entry(&p("out/assets/minecraft")).unwrap().1, None);
}

#[test]
fn removing_a_tree_like_std_does_leaves_nothing_behind() {
    // std's remove_dir_all on Horizon: list, unlink what isn't a folder, recurse, rmdir.
    let links = overlay();
    for name in links.names(&p("out/assets")) {
        links.unlink(&p(&format!("out/assets/{name}"))).unwrap();
    }
    links.rmdir(&p("out/assets")).unwrap();
    assert!(
        lock(&links.store)
            .files
            .keys()
            .all(|f| !f.starts_with("sdmc:/switch/horizon/out/"))
    );
}
