//! `[[sources]]` helpers: per-type metadata, id validation, deterministic ids and
//! [`normalize_sources`].
//!
//! Ids are generated deterministically from `(type, location)`, so two processes that migrate the
//! same file at the same time write the same ids. Only the UI's Add uses random ids.

use std::collections::HashSet;

use crate::schema::{SourceEntry, SourceType};

/// Longest valid source id.
pub const MAX_ID_LEN: usize = 32;

/// Longest source name, in characters (not bytes); longer names are cut.
pub const MAX_NAME_LEN: usize = 64;

impl SourceType {
    /// Whether the source downloads from the internet into `downloads/<id>/`.
    pub fn is_online(self) -> bool {
        matches!(
            self,
            SourceType::Wallhaven
                | SourceType::Unsplash
                | SourceType::Reddit
                | SourceType::Bing
                | SourceType::Apod
                | SourceType::Web
        )
    }

    /// Whether the source needs a `location` (keywords, URL, subreddit, link or path). Bing and APOD
    /// don't; an unsupported row keeps whatever it had.
    pub fn takes_input(self) -> bool {
        !matches!(self, SourceType::Bing | SourceType::Apod)
    }

    /// Prefix of generated ids, e.g. `wh` in `wh-1a2b`.
    pub fn prefix(self) -> &'static str {
        match self {
            SourceType::Wallhaven => "wh",
            SourceType::Unsplash => "us",
            SourceType::Reddit => "rd",
            SourceType::Bing => "bing",
            SourceType::Apod => "apod",
            SourceType::Web => "web",
            SourceType::Folder => "dir",
            SourceType::Images => "img",
            SourceType::AlbumName => "aln",
            SourceType::AlbumDate => "ald",
            SourceType::Unsupported => "unk",
        }
    }

    /// Human-readable name for logs and messages.
    pub fn label(self) -> &'static str {
        match self {
            SourceType::Wallhaven => "Wallhaven",
            SourceType::Unsplash => "Unsplash",
            SourceType::Reddit => "Reddit",
            SourceType::Bing => "Bing",
            SourceType::Apod => "APOD",
            SourceType::Web => "Web page",
            SourceType::Folder => "Folder",
            SourceType::Images => "Images",
            SourceType::AlbumName => "Sequential album by file name",
            SourceType::AlbumDate => "Sequential album by file date",
            SourceType::Unsupported => "Unknown",
        }
    }
}

/// Whether `id` is `[a-z0-9-]{1,32}`: safe as a single path component and stable in TOML.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `"{prefix}-{hex4}"` from a 64-bit FNV-1a hash of `"{type}\0{location}"`. If that id is in
/// `taken`, appends `-2`, `-3`, … until it's free. Does not insert into `taken`.
pub fn deterministic_id(kind: SourceType, location: &str, taken: &HashSet<String>) -> String {
    let hash = fnv1a64(format!("{}\0{location}", kind.as_str()).as_bytes());
    let hex = format!("{hash:016x}");
    let base = format!("{}-{}", kind.prefix(), &hex[..4]);
    if !taken.contains(&base) {
        return base;
    }
    (2u32..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken.contains(id))
        .expect("an unbounded suffix range always finds a free id")
}

/// `name` as stored: surrounding whitespace trimmed and cut to [`MAX_NAME_LEN`] characters.
pub fn normalize_name(name: &str) -> String {
    name.trim()
        .chars()
        .take(MAX_NAME_LEN)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// Replaces empty, invalid and duplicate ids with [`deterministic_id`]s, in row order (the first
/// row keeps a duplicated id), and tidies each name with [`normalize_name`]. Returns one warning
/// per replaced id.
pub fn normalize_sources(sources: &mut [SourceEntry]) -> Vec<String> {
    for src in sources.iter_mut() {
        src.name = normalize_name(&src.name);
    }
    let mut warnings = Vec::new();
    // Valid ids are reserved up front so a generated id never steals a later row's own id.
    let mut taken: HashSet<String> = sources
        .iter()
        .filter(|s| valid_id(&s.id))
        .map(|s| s.id.clone())
        .collect();
    let mut seen = HashSet::new();
    for (i, src) in sources.iter_mut().enumerate() {
        if valid_id(&src.id) && seen.insert(src.id.clone()) {
            continue;
        }
        let new_id = deterministic_id(src.kind, &src.location, &taken);
        let why = if src.id.is_empty() {
            "missing".to_owned()
        } else if valid_id(&src.id) {
            format!("duplicate id {:?}", src.id)
        } else {
            format!("invalid id {:?}", src.id)
        };
        warnings.push(format!(
            "sources[{i}] ({} {:?}): {why}; using {new_id:?}",
            src.kind, src.location
        ));
        taken.insert(new_id.clone());
        seen.insert(new_id.clone());
        src.id = new_id;
    }
    warnings
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes
        .iter()
        .fold(OFFSET, |h, &b| (h ^ u64::from(b)).wrapping_mul(PRIME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, kind: SourceType, location: &str) -> SourceEntry {
        SourceEntry {
            id: id.to_owned(),
            enabled: true,
            kind,
            location: location.to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn valid_id_accepts_lowercase_digits_and_dashes() {
        assert!(valid_id("wh-1a2b"));
        assert!(valid_id("bing"));
        assert!(valid_id(&"a".repeat(32)));
    }

    #[test]
    fn valid_id_rejects_paths_empty_uppercase_and_long() {
        for bad in ["../x", "", "A", "a/b", "a b", "a_b", "é"] {
            assert!(!valid_id(bad), "{bad:?}");
        }
        assert!(!valid_id(&"a".repeat(33)));
    }

    #[test]
    fn fnv1a64_matches_reference_vectors() {
        // Published FNV-1a 64-bit test vectors.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn deterministic_id_is_stable_and_prefixed() {
        let empty = HashSet::new();
        let a = deterministic_id(SourceType::Wallhaven, "nature", &empty);
        let b = deterministic_id(SourceType::Wallhaven, "nature", &empty);
        assert_eq!(a, b);
        assert!(a.starts_with("wh-"), "{a}");
        assert_eq!(a.len(), "wh-".len() + 4);
        assert!(valid_id(&a));
        let other = deterministic_id(SourceType::Wallhaven, "space", &empty);
        assert_ne!(a, other);
    }

    #[test]
    fn deterministic_id_hashes_type_and_location() {
        let empty = HashSet::new();
        let hash = fnv1a64(b"wallhaven\0nature");
        let want = format!("wh-{}", &format!("{hash:016x}")[..4]);
        assert_eq!(
            deterministic_id(SourceType::Wallhaven, "nature", &empty),
            want
        );
    }

    #[test]
    fn deterministic_id_appends_a_suffix_on_collision() {
        let mut taken = HashSet::new();
        let first = deterministic_id(SourceType::Reddit, "earthporn", &taken);
        taken.insert(first.clone());
        let second = deterministic_id(SourceType::Reddit, "earthporn", &taken);
        assert_eq!(second, format!("{first}-2"));
        taken.insert(second);
        let third = deterministic_id(SourceType::Reddit, "earthporn", &taken);
        assert_eq!(third, format!("{first}-3"));
        assert!(valid_id(&third));
    }

    #[test]
    fn every_type_has_a_valid_id_prefix() {
        let empty = HashSet::new();
        for kind in SourceType::ALL {
            let id = deterministic_id(kind, "x", &empty);
            assert!(valid_id(&id), "{kind}: {id}");
            assert!(id.starts_with(&format!("{}-", kind.prefix())));
            assert!(!kind.label().is_empty());
        }
    }

    #[test]
    fn web_type_has_its_config_string_label_and_prefix() {
        assert_eq!(SourceType::from_config_str("web"), SourceType::Web);
        assert_eq!(SourceType::Web.as_str(), "web");
        assert_eq!(SourceType::Web.label(), "Web page");
        assert_eq!(SourceType::Web.prefix(), "web");
    }

    #[test]
    fn from_config_str_round_trips_every_known_type_and_maps_unknown_to_unsupported() {
        for kind in SourceType::ALL {
            assert_eq!(SourceType::from_config_str(kind.as_str()), kind, "{kind}");
        }
        for unknown in ["flickr", "Web", "web-page", ""] {
            assert_eq!(
                SourceType::from_config_str(unknown),
                SourceType::Unsupported,
                "{unknown:?}"
            );
        }
    }

    #[test]
    fn is_online_and_takes_input_for_every_type() {
        use SourceType::*;
        for (kind, online, input) in [
            (Wallhaven, true, true),
            (Unsplash, true, true),
            (Reddit, true, true),
            (Bing, true, false),
            (Apod, true, false),
            (Web, true, true),
            (Folder, false, true),
            (Images, false, true),
            (AlbumName, false, true),
            (AlbumDate, false, true),
            (Unsupported, false, true),
        ] {
            assert_eq!(kind.is_online(), online, "{kind} is_online");
            assert_eq!(kind.takes_input(), input, "{kind} takes_input");
            // Local types are exactly the ones whose location is a path.
            assert_eq!(
                kind.is_local(),
                !online && kind != Unsupported,
                "{kind} is_local"
            );
        }
    }

    #[test]
    fn normalize_sources_keeps_valid_unique_ids() {
        let mut rows = vec![
            row("bing", SourceType::Bing, ""),
            row("wh-nature", SourceType::Wallhaven, "nature"),
        ];
        let before = rows.clone();
        assert!(normalize_sources(&mut rows).is_empty());
        assert_eq!(rows, before);
    }

    #[test]
    fn normalize_sources_replaces_empty_invalid_and_duplicate_ids() {
        let mut rows = vec![
            row("", SourceType::Wallhaven, "nature"),
            row("../evil", SourceType::Folder, "/pics"),
            row("bing", SourceType::Bing, ""),
            row("bing", SourceType::Apod, ""),
        ];
        let warnings = normalize_sources(&mut rows);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings[0].contains("missing"), "{}", warnings[0]);
        assert!(warnings[1].contains("invalid"), "{}", warnings[1]);
        assert!(warnings[2].contains("duplicate"), "{}", warnings[2]);

        assert!(rows[0].id.starts_with("wh-"));
        assert!(rows[1].id.starts_with("dir-"));
        assert_eq!(rows[2].id, "bing", "the first row keeps a duplicated id");
        assert!(rows[3].id.starts_with("apod-"));
        let ids: HashSet<_> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids.len(), rows.len());
        assert!(rows.iter().all(|r| valid_id(&r.id)));
    }

    #[test]
    fn normalize_sources_is_deterministic_and_idempotent() {
        let make = || {
            vec![
                row("", SourceType::Reddit, "earthporn"),
                row("", SourceType::Reddit, "earthporn"),
            ]
        };
        let mut a = make();
        let mut b = make();
        normalize_sources(&mut a);
        normalize_sources(&mut b);
        assert_eq!(a, b);
        assert_eq!(a[1].id, format!("{}-2", a[0].id));
        let again = a.clone();
        assert!(normalize_sources(&mut a).is_empty());
        assert_eq!(a, again);
    }

    #[test]
    fn generated_ids_never_take_a_later_rows_id() {
        let empty = HashSet::new();
        let natural = deterministic_id(SourceType::Wallhaven, "nature", &empty);
        let mut rows = vec![
            row("", SourceType::Wallhaven, "nature"),
            row(&natural, SourceType::Wallhaven, "other"),
        ];
        normalize_sources(&mut rows);
        assert_eq!(rows[1].id, natural);
        assert_eq!(rows[0].id, format!("{natural}-2"));
    }
}
