//! What the index holds of a work, of an episode and of a person: the rows
//! written for each, worked out from what the catalogue holds of them.

use std::collections::HashSet;

use sha2::{Digest as _, Sha256};

use super::{Kind, norm::norm};

/// One row of the index, as it is written.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: i64,
    pub kind: Kind,
    pub ref_id: String,
    pub work_id: Option<String>,
    pub lang: Option<String>,
    pub norm: String,
    pub text: String,
    pub popularity: Option<f64>,
    pub year: Option<i32>,
}

/// The number a row is filed under: what it is — its kind, what it names, its
/// folded text — hashed, with the kind in the top bits. The same row is
/// written again under the same number, so a work refreshed keeps its rows'
/// places in the FTS5 index, and a copy of the table into another database
/// carries no counter that would have to be put right there.
///
/// Fifty-nine bits of SHA-256: two of a million rows of one kind share one
/// with odds of about one in a million, and the second is then not written —
/// one name of one work not found, against an index that never has to ask
/// the database for a number.
pub fn entry_id(kind: Kind, ref_id: &str, norm: &str) -> i64 {
    let digest = Sha256::new()
        .chain_update(kind.as_str())
        .chain_update([0x1f])
        .chain_update(ref_id)
        .chain_update([0x1f])
        .chain_update(norm)
        .finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    let low = u64::from_be_bytes(bytes) & ((1u64 << Kind::SHIFT) - 1);
    (kind.code() << Kind::SHIFT) | low as i64
}

/// What the catalogue holds of a work that it is found by, as read for the
/// index.
#[derive(Clone, Debug, Default)]
pub struct WorkNames {
    pub id: String,
    pub title: String,
    pub sort_title: Option<String>,
    pub original_title: Option<String>,
    pub original_language: Option<String>,
    pub year: Option<i32>,
    pub popularity: Option<f64>,
    /// Its alternative titles, with the language each is filed under.
    pub alternatives: Vec<(String, Option<String>)>,
    /// The alternative titles a person hid, as their locks name them:
    /// the language they are filed under, empty for none, and the title.
    pub hidden: HashSet<(String, String)>,
    /// Its title in each language a translation is held in.
    pub translations: Vec<(String, String)>,
    /// The titles locked by hand — the title, the original or the sort title
    /// — each with the language it was locked in, empty for every language.
    pub locked: Vec<(String, String)>,
    pub episodes: Vec<EpisodeName>,
}

/// An episode, as the index names it.
#[derive(Clone, Debug, Default)]
pub struct EpisodeName {
    pub id: String,
    pub title: String,
    /// The title locked by hand, where one is.
    pub locked: Option<String>,
    /// `YYYY-MM-DD`, or as much of it as is known.
    pub air_date: Option<String>,
}

/// The rows of a work: one for each distinct name it goes by, and one for each
/// of its episodes' titles that names something.
pub fn work_entries(work: &WorkNames) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |entries: &mut Vec<Entry>, text: &str, lang: Option<&str>| {
        let folded = norm(text);
        if folded.is_empty() || !seen.insert(folded.clone()) {
            return;
        }
        entries.push(Entry {
            id: entry_id(Kind::Work, &work.id, &folded),
            kind: Kind::Work,
            ref_id: work.id.clone(),
            work_id: Some(work.id.clone()),
            lang: lang.filter(|l| !l.is_empty()).map(str::to_string),
            norm: folded,
            text: text.trim().to_string(),
            popularity: work.popularity,
            year: work.year,
        });
    };

    // The title first, then the names nearest it: two that fold alike are one
    // row, and the one kept should be the name a reader knows.
    add(&mut entries, &work.title, None);
    if let Some(original) = &work.original_title {
        add(&mut entries, original, work.original_language.as_deref());
    }
    if let Some(sort) = &work.sort_title {
        add(&mut entries, sort, None);
    }
    // A work renamed by hand is found under the name it was given.
    for (value, lang) in &work.locked {
        add(&mut entries, value, Some(lang));
    }
    for (title, lang) in &work.translations {
        add(&mut entries, title, Some(lang));
    }
    for (title, lang) in &work.alternatives {
        let filed = (lang.clone().unwrap_or_default(), title.clone());
        if work.hidden.contains(&filed) {
            continue;
        }
        add(&mut entries, title, lang.as_deref());
    }

    for episode in &work.episodes {
        let mut named: HashSet<String> = HashSet::new();
        for title in episode.locked.iter().chain([&episode.title]) {
            let folded = norm(title);
            if folded.is_empty() || is_placeholder(&folded) || !named.insert(folded.clone()) {
                continue;
            }
            entries.push(Entry {
                id: entry_id(Kind::Episode, &episode.id, &folded),
                kind: Kind::Episode,
                ref_id: episode.id.clone(),
                work_id: Some(work.id.clone()),
                lang: None,
                norm: folded,
                text: title.trim().to_string(),
                popularity: work.popularity,
                year: episode
                    .air_date
                    .as_deref()
                    .and_then(|d| d.get(..4))
                    .and_then(|y| y.parse().ok()),
            });
        }
    }

    entries
}

/// Whether an episode's title says nothing of it: `Episode 12`, `Folge 3`,
/// `TBA`. Thousands of them would fill the index and answer nobody.
fn is_placeholder(folded: &str) -> bool {
    const SAID_NOTHING: [&str; 5] = ["tba", "tbd", "tbc", "untitled", "sans titre"];
    const NUMBERED: [&str; 12] = [
        "episode",
        "episodio",
        "episodi",
        "folge",
        "capitulo",
        "aflevering",
        "afsnit",
        "avsnitt",
        "jakso",
        "odcinek",
        "epizod",
        "epizoda",
    ];
    if SAID_NOTHING.contains(&folded) || folded.chars().all(|c| c.is_numeric() || c == ' ') {
        return true;
    }
    match folded.split_once(' ') {
        Some((word, number)) => {
            NUMBERED.contains(&word) && number.chars().all(|c| c.is_numeric() || c == ' ')
        }
        None => false,
    }
}

/// Somebody credited on works of the catalogue, as the index names them:
/// their record, every name they are found by, and how many works credit
/// them by a credit no lock hides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonNames {
    /// Their record's id.
    pub id: String,
    /// Their name first, then the one in their own script and the others
    /// they go by.
    pub names: Vec<String>,
    pub works: i64,
}

/// The rows of somebody credited: one for each distinct name they go by,
/// each worth as many works as credit them. None for somebody no work
/// credits, nor for a name that folds to nothing.
pub fn person_entries(person: &PersonNames) -> Vec<Entry> {
    if person.works <= 0 {
        return Vec::new();
    }
    let mut seen: HashSet<String> = HashSet::new();
    person
        .names
        .iter()
        .filter_map(|name| {
            let folded = norm(name);
            if folded.is_empty() || !seen.insert(folded.clone()) {
                return None;
            }
            Some(Entry {
                id: entry_id(Kind::Person, &person.id, &folded),
                kind: Kind::Person,
                ref_id: person.id.clone(),
                work_id: None,
                lang: None,
                norm: folded,
                text: name.trim().to_string(),
                popularity: Some(person.works as f64),
                year: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rows_number_is_its_kinds_and_does_not_change() {
        let id = entry_id(Kind::Work, "w1", "breaking bad");
        assert_eq!(id, entry_id(Kind::Work, "w1", "breaking bad"));
        assert_ne!(id, entry_id(Kind::Work, "w2", "breaking bad"));
        for kind in [Kind::Work, Kind::Person, Kind::Episode] {
            let (from, to) = kind.ids();
            let id = entry_id(kind, "x", "y");
            assert!(from <= id && id < to, "{kind:?}: {id}");
        }
    }

    #[test]
    fn a_work_is_found_by_each_of_its_names_once_and_not_by_a_hidden_one() {
        let work = WorkNames {
            id: "w".into(),
            title: "Money Heist".into(),
            original_title: Some("La casa de papel".into()),
            original_language: Some("es".into()),
            alternatives: vec![
                ("La Casa de Papel".into(), Some("es".into())),
                ("Haus des Geldes".into(), Some("de".into())),
                ("A wrong title".into(), None),
            ],
            hidden: [(String::new(), "A wrong title".to_string())].into(),
            translations: vec![("La Casa de Papel".into(), "fra".into())],
            locked: vec![("Le Braquage".into(), "fra".into())],
            ..Default::default()
        };
        let names: Vec<(String, Option<String>)> = work_entries(&work)
            .into_iter()
            .map(|e| (e.norm, e.lang))
            .collect();
        assert_eq!(
            names,
            [
                ("money heist".to_string(), None),
                ("la casa de papel".to_string(), Some("es".to_string())),
                ("le braquage".to_string(), Some("fra".to_string())),
                ("haus des geldes".to_string(), Some("de".to_string())),
            ]
        );
    }

    #[test]
    fn an_episode_titled_by_its_number_is_not_indexed() {
        let work = WorkNames {
            id: "w".into(),
            title: "Show".into(),
            episodes: vec![
                EpisodeName {
                    id: "e1".into(),
                    title: "Ozymandias".into(),
                    air_date: Some("2013-09-15".into()),
                    ..Default::default()
                },
                EpisodeName {
                    id: "e2".into(),
                    title: "Episode 12".into(),
                    ..Default::default()
                },
                EpisodeName {
                    id: "e3".into(),
                    title: "Épisode 3".into(),
                    ..Default::default()
                },
                EpisodeName {
                    id: "e4".into(),
                    title: "TBA".into(),
                    locked: Some("The Hand-Picked Name".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let episodes: Vec<(String, String, Option<i32>)> = work_entries(&work)
            .into_iter()
            .filter(|e| e.kind == Kind::Episode)
            .map(|e| (e.ref_id, e.text, e.year))
            .collect();
        assert_eq!(
            episodes,
            [
                ("e1".to_string(), "Ozymandias".to_string(), Some(2013)),
                ("e4".to_string(), "The Hand-Picked Name".to_string(), None),
            ]
        );
    }

    #[test]
    fn a_person_is_found_by_each_of_their_names_once_and_not_without_a_work() {
        let person = PersonNames {
            id: "p1".into(),
            names: vec![
                "Wim Wenders".into(),
                "WIM WENDERS".into(),
                "Ernst Wilhelm Wenders".into(),
                " ".into(),
            ],
            works: 3,
        };
        let rows = person_entries(&person);
        let names: Vec<&str> = rows.iter().map(|e| e.norm.as_str()).collect();
        assert_eq!(names, ["wim wenders", "ernst wilhelm wenders"]);
        assert!(
            rows.iter()
                .all(|e| e.ref_id == "p1" && e.popularity == Some(3.0))
        );
        assert_ne!(rows[0].id, rows[1].id);

        let uncredited = PersonNames { works: 0, ..person };
        assert!(person_entries(&uncredited).is_empty());
    }
}
