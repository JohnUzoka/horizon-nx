//! Non-ASCII file names on the Switch's SD card, kept in ASCII.
//!
//! The FS service can't hold a non-ASCII name. It reads a written path's bytes as Shift-JIS, so
//! a world named `Мир` (UTF-8 `D0 9C D0 B8 D1 80`) was saved in a folder named `ﾐ慴ｸﾑ_`, and
//! its listings leave out every entry whose name isn't ASCII, so that world never showed in the
//! world list (KoHaku743/horizon-nx issue #7; its console probes of 24 and 25 November 2026).
//!
//! Each path part with a non-ASCII character goes to the card as `xn--` and its Punycode
//! (RFC 3492, as an internationalised domain name), and a listed name in that form comes back
//! as the name it stands for: the game sees `Мир`, the card holds `xn--o0a0dya`. ASCII paths,
//! which are nearly all of them, go through untouched.
//!
//! A folder an earlier build wrote under a raw non-ASCII name is still on the card, hidden from
//! listings but found by that raw name (the FS service mangles it the same way again). The first
//! time a path reaches one by name, [`adopt_legacy`] renames it to its ASCII form, after which it
//! lists: opening the world by name (`--world Мир`) or naming a new world the same brings it back.
//!
//! The SD card's newlib device does all this for every file the game opens (`device.rs`); the
//! logic here is plain Rust, tested on any host.

#[cfg(target_os = "horizon")]
mod device;
#[cfg(target_os = "horizon")]
pub(crate) use device::install;

use std::borrow::Cow;

const PREFIX: &str = "xn--";

/// `path` as it goes to the SD card: each part with a non-ASCII character in its ASCII form.
pub(crate) fn to_fs(path: &str) -> Cow<'_, str> {
    if path.is_ascii() {
        return Cow::Borrowed(path);
    }
    let parts: Vec<Cow<'_, str>> = path
        .split('/')
        .map(|part| match encode(part) {
            Some(ascii) if !part.is_ascii() => Cow::Owned(format!("{PREFIX}{ascii}")),
            _ => Cow::Borrowed(part),
        })
        .collect();
    Cow::Owned(parts.join("/"))
}

/// The name a listed name stands for, when it's one [`to_fs`] makes; `None` for any other name,
/// which lists as it is. A form `to_fs` wouldn't make from what it decodes to (a name typed by
/// hand, say) stays as it is too: listed decoded, it couldn't be opened under that name.
pub(crate) fn from_fs(listed: &[u8]) -> Option<String> {
    if listed.len() <= PREFIX.len() || !listed[..PREFIX.len()].eq_ignore_ascii_case(PREFIX.as_bytes()) {
        return None;
    }
    let rest = core::str::from_utf8(&listed[PREFIX.len()..]).ok()?;
    let decoded = decode(rest)?;
    (!decoded.is_ascii() && encode(&decoded)?.eq_ignore_ascii_case(rest)).then_some(decoded)
}

/// Renames each part of `path` that an earlier build wrote under its raw non-ASCII name (one
/// that `exists` finds by that name but not in its ASCII form) to the ASCII form [`to_fs`] gives
/// it. `exists` and `rename` take card paths; the parts before the one renamed are already in
/// their card form, as everything is once its parents are.
pub(crate) fn adopt_legacy(
    path: &str,
    mut exists: impl FnMut(&str) -> bool,
    mut rename: impl FnMut(&str, &str),
) {
    let mut card = String::with_capacity(path.len() * 2);
    for (i, part) in path.split('/').enumerate() {
        if i > 0 {
            card.push('/');
        }
        let Some(ascii) = (!part.is_ascii()).then(|| encode(part)).flatten() else {
            card.push_str(part);
            continue;
        };
        let raw = format!("{card}{part}");
        card.push_str(PREFIX);
        card.push_str(&ascii);
        if !exists(&card) && exists(&raw) {
            rename(&raw, &card);
        }
    }
}

const BASE: u32 = 36;
const T_MIN: u32 = 1;
const T_MAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;
const INITIAL_BIAS: u32 = 72;
const INITIAL_N: u32 = 128;

fn adapt(delta: u32, points: u32, first: bool) -> u32 {
    let mut delta = if first { delta / DAMP } else { delta / 2 };
    delta += delta / points;
    let mut k = 0;
    while delta > ((BASE - T_MIN) * T_MAX) / 2 {
        delta /= BASE - T_MIN;
        k += BASE;
    }
    k + (BASE - T_MIN + 1) * delta / (delta + SKEW)
}

fn threshold(k: u32, bias: u32) -> u32 {
    if k <= bias {
        T_MIN
    } else if k >= bias + T_MAX {
        T_MAX
    } else {
        k - bias
    }
}

fn digit(d: u32) -> char {
    (if d < 26 { b'a' + d as u8 } else { b'0' + (d - 26) as u8 }) as char
}

/// RFC 3492's encoding of `input`.
fn encode(input: &str) -> Option<String> {
    let points: Vec<u32> = input.chars().map(u32::from).collect();
    let mut out: String = input.chars().filter(char::is_ascii).collect();
    let basic = out.len() as u32;
    let mut handled = basic;
    if basic > 0 {
        out.push('-');
    }
    let (mut n, mut delta, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    let total = points.len() as u32;
    while handled < total {
        let m = *points.iter().filter(|&&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(handled + 1)?)?;
        n = m;
        for &c in &points {
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = threshold(k, bias);
                    if q < t {
                        break;
                    }
                    out.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n += 1;
    }
    Some(out)
}

/// RFC 3492's decoding; `None` for anything that isn't a valid encoding.
fn decode(input: &str) -> Option<String> {
    let (basic, rest) = match input.rfind('-') {
        Some(i) => (&input[..i], &input[i + 1..]),
        None => ("", input),
    };
    if !basic.is_ascii() {
        return None;
    }
    let mut out: Vec<char> = basic.chars().collect();
    let (mut n, mut i, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    let mut bytes = rest.bytes().peekable();
    while bytes.peek().is_some() {
        let old = i;
        let mut w = 1u32;
        let mut k = BASE;
        loop {
            let d = match bytes.next()? {
                b @ b'a'..=b'z' => b - b'a',
                b @ b'A'..=b'Z' => b - b'A',
                b @ b'0'..=b'9' => b - b'0' + 26,
                _ => return None,
            } as u32;
            i = i.checked_add(d.checked_mul(w)?)?;
            let t = threshold(k, bias);
            if d < t {
                break;
            }
            w = w.checked_mul(BASE - t)?;
            k += BASE;
        }
        let len = out.len() as u32 + 1;
        bias = adapt(i - old, len, old == 0);
        n = n.checked_add(i / len)?;
        i %= len;
        out.insert(i as usize, char::from_u32(n)?);
        i += 1;
    }
    Some(out.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::{adopt_legacy, from_fs, to_fs};
    use std::collections::BTreeMap;

    /// Expected forms from Python's `str.encode("punycode")`, an independent RFC 3492 encoder.
    #[test]
    fn non_ascii_names_go_down_in_ascii_and_come_back_as_they_were() {
        let cases = [
            ("Мир", "xn--o0a0dya"),
            ("Новый мир (1)", "xn--  (1)-use1mzbh5as3a2l"),
            ("Base Мир", "xn--Base -tse0rmc"),
            ("日本語のワールド", "xn--u9j3jrdpa9d7726bo4bc84j"),
            ("Café", "xn--Caf-dma"),
            ("😀 emoji", "xn-- emoji-2v74e"),
        ];
        for (name, on_card) in cases {
            assert_eq!(to_fs(name), on_card, "{name}");
            assert_eq!(from_fs(on_card.as_bytes()).as_deref(), Some(name), "{name}");
        }
        // Only the parts that need it, and ASCII paths untouched.
        assert_eq!(
            to_fs("sdmc:/switch/horizon/saves/Мир/level.dat"),
            "sdmc:/switch/horizon/saves/xn--o0a0dya/level.dat"
        );
        assert_eq!(to_fs("saves/Мир"), "saves/xn--o0a0dya");
        assert!(matches!(
            to_fs("sdmc:/switch/horizon/saves/New World"),
            std::borrow::Cow::Borrowed("sdmc:/switch/horizon/saves/New World")
        ));
        // Plain names, and names that only look like the form, list as they are.
        assert_eq!(from_fs(b"New World"), None);
        assert_eq!(from_fs(b"xn--"), None);
        assert_eq!(from_fs(b"xn--abc-"), None);
        assert_eq!(from_fs(b"xn--!!"), None);
        assert_eq!(from_fs("ﾐ慴ｸﾑ_".as_bytes()), None);
        // FAT keeps a name's case as typed but matches any: an upper-case form still lists.
        assert_eq!(from_fs(b"XN--O0A0DYA").as_deref(), Some("Мир"));
    }

    /// The Switch's SD card as the FS service shows it (issue #7; a console, 27 November 2026):
    /// a path's bytes are read as Shift-JIS, which turns UTF-8 into other characters (here: each
    /// non-ASCII byte as a halfwidth katakana, enough to tell names apart) the same way on every
    /// call, so a raw name still finds what it wrote, and a listing leaves out every name that
    /// isn't ASCII.
    #[derive(Default)]
    struct Card {
        /// Every folder (`None`) and file on the card, by the path the card holds.
        entries: BTreeMap<String, Option<Vec<u8>>>,
    }

    impl Card {
        fn held(path: &str) -> String {
            let katakana = |b: u8| char::from_u32(0xFF61 + u32::from(b & 0x3F)).unwrap();
            path.bytes().map(|b| if b.is_ascii() { b as char } else { katakana(b) }).collect()
        }
        fn mkdir(&mut self, path: &str) {
            self.entries.insert(Self::held(path), None);
        }
        fn write(&mut self, path: &str, data: &[u8]) {
            self.entries.insert(Self::held(path), Some(data.to_vec()));
        }
        fn read(&self, path: &str) -> Option<&[u8]> {
            self.entries.get(&Self::held(path))?.as_deref()
        }
        fn exists(&self, path: &str) -> bool {
            self.entries.contains_key(&Self::held(path))
        }
        fn rename(&mut self, from: &str, to: &str) {
            let (from, to) = (Self::held(from), Self::held(to));
            let moved: Vec<String> = self
                .entries
                .keys()
                .filter(|p| **p == from || p.starts_with(&format!("{from}/")))
                .cloned()
                .collect();
            for path in moved {
                let entry = self.entries.remove(&path).unwrap();
                self.entries.insert(format!("{to}{}", &path[from.len()..]), entry);
            }
        }
        fn list(&self, dir: &str) -> Vec<Vec<u8>> {
            let below = format!("{}/", Self::held(dir));
            self.entries
                .keys()
                .filter_map(|p| p.strip_prefix(&below).filter(|name| !name.contains('/')))
                .filter(|name| name.is_ascii())
                .map(|name| name.as_bytes().to_vec())
                .collect()
        }
    }

    /// The game's view through the names layer, as the SD card's device gives it.
    struct Game<'a>(&'a mut Card);

    impl Game<'_> {
        /// What the device does before handing libnx a path.
        fn card(&mut self, path: &str) -> String {
            let card = std::cell::RefCell::new(&mut *self.0);
            adopt_legacy(path, |p| card.borrow().exists(p), |from, to| card.borrow_mut().rename(from, to));
            to_fs(path).into_owned()
        }
        fn mkdir(&mut self, path: &str) {
            let path = self.card(path);
            self.0.mkdir(&path);
        }
        fn write(&mut self, path: &str, data: &[u8]) {
            let path = self.card(path);
            self.0.write(&path, data);
        }
        fn read(&mut self, path: &str) -> Option<Vec<u8>> {
            let path = self.card(path);
            self.0.read(&path).map(<[u8]>::to_vec)
        }
        fn exists(&mut self, path: &str) -> bool {
            let path = self.card(path);
            self.0.exists(&path)
        }
        fn list(&mut self, path: &str) -> Vec<String> {
            let path = self.card(path);
            let mut names: Vec<String> = self
                .0
                .list(&path)
                .iter()
                .map(|n| from_fs(n).unwrap_or_else(|| String::from_utf8(n.clone()).unwrap()))
                .collect();
            names.sort();
            names
        }
    }

    const SAVES: &str = "sdmc:/switch/horizon/saves";

    fn card_with_saves() -> Card {
        let mut card = Card::default();
        card.mkdir(SAVES);
        card
    }

    #[test]
    fn a_world_named_in_cyrillic_is_created_listed_and_reopened() {
        let mut card = card_with_saves();
        let mut game = Game(&mut card);
        for name in ["Мир", "Новый мир (1)", "New World"] {
            game.mkdir(&format!("{SAVES}/{name}"));
            game.write(&format!("{SAVES}/{name}/level.dat"), name.as_bytes());
        }
        // The world list sees each world under its own name, and each opens again.
        assert_eq!(game.list(SAVES), ["New World", "Мир", "Новый мир (1)"]);
        for name in game.list(SAVES) {
            let level = game.read(&format!("{SAVES}/{name}/level.dat"));
            assert_eq!(level.as_deref(), Some(name.as_bytes()));
        }
        // Everything on the card is ASCII, and the ASCII world is where it always was.
        assert!(card.entries.keys().all(|p| p.is_ascii()), "{:?}", card.entries.keys());
        assert_eq!(card.read(&format!("{SAVES}/New World/level.dat")), Some(&b"New World"[..]));
        assert_eq!(card.read(&format!("{SAVES}/xn--o0a0dya/level.dat")), Some(&"Мир".as_bytes()[..]));
    }

    #[test]
    fn a_world_an_earlier_build_saved_under_its_raw_name_opens_and_lists_again() {
        let mut card = card_with_saves();
        // 0.2.1: the folder is written under the raw name, and no listing shows it.
        card.mkdir(&format!("{SAVES}/Старый"));
        card.write(&format!("{SAVES}/Старый/level.dat"), b"old");
        card.mkdir(&format!("{SAVES}/Старый/region"));
        card.write(&format!("{SAVES}/Старый/region/r.0.0.mca"), b"chunks");
        assert!(card.list(SAVES).is_empty());

        let mut game = Game(&mut card);
        assert_eq!(game.list(SAVES), Vec::<String>::new());
        // Asked for by name (`--world Старый`, or a new world named the same, whose folder
        // search asks whether it exists), it is found, renamed to its ASCII form, and lists.
        assert!(game.exists(&format!("{SAVES}/Старый")));
        assert_eq!(game.list(SAVES), ["Старый"]);
        assert_eq!(game.read(&format!("{SAVES}/Старый/level.dat")).as_deref(), Some(&b"old"[..]));
        let region = game.read(&format!("{SAVES}/Старый/region/r.0.0.mca"));
        assert_eq!(region.as_deref(), Some(&b"chunks"[..]));
        assert!(card.entries.keys().all(|p| p.is_ascii()), "{:?}", card.entries.keys());

        // A folder already in its ASCII form is left alone, even with a raw one beside it.
        let mut card = card_with_saves();
        card.mkdir(&format!("{SAVES}/xn--o0a0dya"));
        card.write(&format!("{SAVES}/xn--o0a0dya/level.dat"), b"new");
        card.mkdir(&format!("{SAVES}/Мир"));
        card.write(&format!("{SAVES}/Мир/level.dat"), b"old");
        let mut game = Game(&mut card);
        assert_eq!(game.read(&format!("{SAVES}/Мир/level.dat")).as_deref(), Some(&b"new"[..]));
        assert_eq!(card.read(&format!("{SAVES}/Мир/level.dat")), Some(&b"old"[..]));
    }

    /// Create World's folder search (horizon-client `title.rs` `target_folder`, through
    /// `world_folder::result_folder`, the same in 0.2.1): the name, then `name (1)`, `name (2)`...
    /// until `exists` says no. Each check goes through the device, so a hidden world of that
    /// name is adopted by the check, and the new world gets the next free folder.
    fn create_world_folder(game: &mut Game<'_>, name: &str) -> String {
        let mut folder = name.to_owned();
        let mut count = 0;
        while game.exists(&format!("{SAVES}/{folder}")) {
            count += 1;
            folder = format!("{name} ({count})");
        }
        game.mkdir(&format!("{SAVES}/{folder}"));
        game.write(&format!("{SAVES}/{folder}/level.dat"), b"new");
        folder
    }

    #[test]
    fn naming_a_new_world_like_a_hidden_one_brings_the_hidden_one_back_untouched() {
        let mut card = card_with_saves();
        // 0.2.1 saved Старый, then a second world named the same in Старый (1), both raw.
        for folder in ["Старый", "Старый (1)"] {
            card.mkdir(&format!("{SAVES}/{folder}"));
            card.write(&format!("{SAVES}/{folder}/level.dat"), folder.as_bytes());
            card.mkdir(&format!("{SAVES}/{folder}/region"));
            card.write(&format!("{SAVES}/{folder}/region/r.0.0.mca"), b"chunks");
        }
        let mut game = Game(&mut card);
        assert_eq!(game.list(SAVES), Vec::<String>::new());

        assert_eq!(create_world_folder(&mut game, "Старый"), "Старый (2)");
        assert_eq!(game.list(SAVES), ["Старый", "Старый (1)", "Старый (2)"]);
        for folder in ["Старый", "Старый (1)"] {
            let level = game.read(&format!("{SAVES}/{folder}/level.dat"));
            assert_eq!(level.as_deref(), Some(folder.as_bytes()));
            let region = game.read(&format!("{SAVES}/{folder}/region/r.0.0.mca"));
            assert_eq!(region.as_deref(), Some(&b"chunks"[..]));
        }
        let level = game.read(&format!("{SAVES}/Старый (2)/level.dat"));
        assert_eq!(level.as_deref(), Some(&b"new"[..]));
        assert!(card.entries.keys().all(|p| p.is_ascii()), "{:?}", card.entries.keys());
    }

    #[test]
    fn ascii_paths_are_never_looked_at() {
        let mut looked = Vec::new();
        adopt_legacy(&format!("{SAVES}/New World/level.dat"), |p| {
            looked.push(p.to_owned());
            true
        }, |_, _| panic!("renamed"));
        assert!(looked.is_empty());
    }
}
