//! The characters of a work, as the merge makes them.
//!
//! TheTVDB lists a series' parts, each by its record of one actor's part;
//! AniList an anime's characters, each by an id of its own and with who
//! voices it in every dub; TMDB names a part in each credit of its cast, and
//! gives it no id. Every source's characters are kept, one character once:
//! two sources' are one when an id both give says so, else when they bear
//! one name — the order of its words, its accents and its case aside, or in
//! its own script — else when one person plays both and nobody else in
//! either list. The first source's character stands, its name and its place,
//! and the others complete it: an id, its name in its own script, the voices
//! of a dub it has none of. AniList's name of a character, its picture of it
//! and its word on what the character is to the work replace TheTVDB's
//! ([`CHARACTER_AUTHORITIES`]): TheTVDB's are typed by whoever filed the
//! part, AniList's are the anime's own.
//!
//! Who plays a character is a credit of the work's cast: the one TheTVDB's
//! part was made into, followed into the credit the merge matched it with
//! (`rules::merge_cast`); else the credit that names the part, as TMDB's
//! do; else, for an anime, the credit of its voice in the work's own
//! language.

use std::collections::BTreeMap;

use super::CHARACTER_AUTHORITIES;
use crate::domain::{
    Credit, CreditType, MediaItem,
    person::{Character, CharacterCredit, People, character_key},
};

/// The language a work's original voices speak when it does not say: the
/// voices are AniList's, and its works are Japanese as often as not.
const ORIGINAL_VOICES: &str = "jpn";

/// Fold another source's characters into the work's, its credits and its
/// people already merged: the credits `renamed` — TheTVDB's matched into
/// the list, by their ids — are followed into the credits they went into,
/// on either side; a credit no longer in the list plays nothing. Each of
/// theirs the same as one of the work's completes it; the others are added
/// after the work's, in their order.
pub(super) fn union(
    into: &mut MediaItem,
    other: Vec<CharacterCredit>,
    renamed: &BTreeMap<String, String>,
) {
    let original = original_language(into);
    let MediaItem {
        characters,
        credits,
        people,
        ..
    } = into;
    let follow = |credit: &mut Option<String>| {
        if let Some(went) = credit.as_ref().and_then(|id| renamed.get(id)) {
            *credit = Some(went.clone());
        }
        if credit
            .as_ref()
            .is_some_and(|id| !credits.iter().any(|c| c.id == *id))
        {
            *credit = None;
        }
    };
    for ours in characters.iter_mut() {
        follow(&mut ours.credit_id);
    }
    let mut theirs = other;
    for given in &mut theirs {
        follow(&mut given.credit_id);
    }
    if theirs.is_empty() {
        return;
    }

    let ours_played: Vec<Vec<String>> = characters
        .iter()
        .map(|c| performer(c, credits, people, &original))
        .collect();
    let theirs_played: Vec<Vec<String>> = theirs
        .iter()
        .map(|c| performer(c, credits, people, &original))
        .collect();

    let held = characters.len();
    let offset = characters
        .iter()
        .map(|c| c.sort_order)
        .max()
        .map_or(0, |m| m.saturating_add(1));
    for (at, mut given) in theirs.into_iter().enumerate() {
        let found = (0..held)
            .find(|&i| same(&characters[i], &given))
            .or_else(|| alone_played(at, &given, characters, &ours_played, &theirs_played));
        match found {
            Some(i) => complete(&mut characters[i], given),
            None => {
                given.sort_order = given.sort_order.saturating_add(offset);
                characters.push(given);
            }
        }
    }
}

/// Give each character no source said who plays it the credit of the cast
/// that does: the one credit of an actor that names the part — TMDB's
/// `Eren Yeager (voice)` — else the one credit of an actor that is its voice
/// in the work's own language. Nobody where none is, or more than one.
pub fn cast(item: &mut MediaItem) {
    let original = original_language(item);
    let MediaItem {
        characters,
        credits,
        people,
        ..
    } = item;
    for character in characters.iter_mut() {
        if character.credit_id.is_some() {
            continue;
        }
        character.credit_id = named_by(character, credits)
            .or_else(|| voiced_by(character, credits, people, &original));
    }
}

/// The characters the sources' answers give, as the merge makes them, played
/// by the credits a work holds (`held`, with whom each names): the sources
/// in the order `priority` puts them, each one's credits matched into the
/// held ones as a merge matches them, only to tell which held credit plays
/// whom. A character played by none of the held credits is played by
/// nobody. What a pass over a work's answers kept makes of their characters
/// (`jobs::refresh::replay_snapshots`).
pub fn onto_held(
    held: (&[Credit], &People),
    original_language: Option<&str>,
    mut sources: Vec<super::Contribution>,
    priority: &[String],
) -> Vec<CharacterCredit> {
    let rank = |provider: &str| {
        priority
            .iter()
            .position(|p| p == provider)
            .unwrap_or(priority.len())
    };
    sources.sort_by_key(|c| rank(&c.provider));

    let mut work = MediaItem::empty(crate::domain::MediaKind::Series);
    work.credits = held.0.to_vec();
    work.people = held.1.clone();
    work.original_language = original_language.map(str::to_string);
    for source in sources {
        let renamed = super::rules::merge_cast(
            &mut work.credits,
            &mut work.people,
            source.item.credits,
            source.item.people,
        );
        union(&mut work, source.item.characters, &renamed);
    }
    cast(&mut work);

    let mut characters = work.characters;
    for character in &mut characters {
        if character
            .credit_id
            .as_ref()
            .is_some_and(|id| !held.0.iter().any(|c| c.id == *id))
        {
            character.credit_id = None;
        }
    }
    characters.sort_by_key(|c| c.sort_order);
    characters
}

/// The language a work's original voices speak: its own, else Japanese.
fn original_language(item: &MediaItem) -> String {
    item.original_language
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or(ORIGINAL_VOICES)
        .to_string()
}

/// A name as names are compared here: its words folded and put in order
/// ([`character_key`]), and — for a name in a script that writes no spaces
/// between them, which one source spaces and another does not — its words
/// run together as they come.
fn keys(name: &str) -> Vec<String> {
    let words = character_key(name);
    if words.is_empty() {
        return Vec::new();
    }
    let run: String = crate::search::norm(name).split_whitespace().collect();
    let mut keys = vec![words.join(" ")];
    if run != keys[0] {
        keys.push(run);
    }
    keys
}

fn meet(a: &[String], b: &[String]) -> bool {
    a.iter().any(|key| b.contains(key))
}

/// The names a character goes by: its own, and in its own script.
fn names_of(character: &Character) -> Vec<String> {
    std::iter::once(character.name.as_str())
        .chain(character.native_name.as_deref())
        .flat_map(keys)
        .collect()
}

/// Whether two sources' characters are one: as an id both give says, else
/// by a name both bear.
fn same(ours: &CharacterCredit, theirs: &CharacterCredit) -> bool {
    match ours.character.same_by_id(&theirs.character) {
        Some(said) => said,
        None => meet(&names_of(&ours.character), &names_of(&theirs.character)),
    }
}

/// The names who plays a character goes by: the person its credit is filed
/// under — their name, and in their own script — and its voices in the work's
/// own language.
fn performer(
    character: &CharacterCredit,
    credits: &[Credit],
    people: &People,
    original: &str,
) -> Vec<String> {
    let mut names: Vec<&str> = Vec::new();
    if let Some(credit) = character
        .credit_id
        .as_ref()
        .and_then(|id| credits.iter().find(|c| c.id == *id))
    {
        names.push(&credit.person_name);
        names.extend(people.of(credit).and_then(|p| p.original_name.as_deref()));
    }
    for voice in character.voices.iter().filter(|v| v.language == original) {
        names.push(&voice.name);
        names.extend(voice.native_name.as_deref());
    }
    names.into_iter().flat_map(keys).collect()
}

/// The work's character `given` is when one person plays both and nobody
/// else in either list, no id saying they are two.
fn alone_played(
    at: usize,
    given: &CharacterCredit,
    characters: &[CharacterCredit],
    ours: &[Vec<String>],
    theirs: &[Vec<String>],
) -> Option<usize> {
    let played = &theirs[at];
    if played.is_empty()
        || theirs
            .iter()
            .enumerate()
            .any(|(i, other)| i != at && meet(other, played))
    {
        return None;
    }
    let mut found = ours
        .iter()
        .enumerate()
        .filter(|(_, by)| meet(by, played))
        .map(|(i, _)| i);
    let (Some(only), None) = (found.next(), found.next()) else {
        return None;
    };
    (characters[only].character.same_by_id(&given.character) != Some(false)).then_some(only)
}

/// What another source knows of the same character that the work's does
/// not; AniList's name, picture and role in place of TheTVDB's. The list's
/// source stays the character's `source`; the other's id, and the voices it
/// names, are what tell later that it gave the character too
/// (`provenance::gave_character`): a refresh it does not answer hands them
/// back rather than lose them.
fn complete(ours: &mut CharacterCredit, theirs: CharacterCredit) {
    let authority =
        |source: Option<&str>| source.is_some_and(|s| CHARACTER_AUTHORITIES.contains(&s));
    if authority(theirs.source.as_deref()) && !authority(ours.source.as_deref()) {
        if !theirs.character.name.trim().is_empty() {
            ours.character.name.clone_from(&theirs.character.name);
        }
        if theirs.character.image.is_some() {
            ours.character.image.clone_from(&theirs.character.image);
        }
        if theirs.role.is_some() {
            ours.role = theirs.role;
        }
    }
    ours.character.complete(&theirs.character);
    ours.role = ours.role.or(theirs.role);
    if ours.credit_id.is_none() {
        ours.credit_id = theirs.credit_id;
    }
    let spoken: Vec<String> = ours.voices.iter().map(|v| v.language.clone()).collect();
    ours.voices.extend(
        theirs
            .voices
            .into_iter()
            .filter(|v| !spoken.contains(&v.language)),
    );
}

/// The one credit of an actor whose part is the character's: a part as a
/// credit names it, one of several TheTVDB puts together (`A / B`), what
/// TMDB writes after a voice's part — `(voice)` — aside.
fn named_by(character: &CharacterCredit, credits: &[Credit]) -> Option<String> {
    let names = names_of(&character.character);
    if names.is_empty() {
        return None;
    }
    let parts = |credit: &Credit| -> Vec<String> {
        credit
            .character_name
            .as_deref()
            .unwrap_or_default()
            .split(" / ")
            .map(without_note)
            .flat_map(keys)
            .collect()
    };
    only(
        credits
            .iter()
            .filter(|c| c.credit_type == CreditType::Actor && meet(&parts(c), &names)),
    )
}

/// The one credit of an actor who is the character's voice in the work's own
/// language.
fn voiced_by(
    character: &CharacterCredit,
    credits: &[Credit],
    people: &People,
    original: &str,
) -> Option<String> {
    let voices: Vec<String> = character
        .voices
        .iter()
        .filter(|v| v.language == original)
        .flat_map(|v| std::iter::once(v.name.as_str()).chain(v.native_name.as_deref()))
        .flat_map(keys)
        .collect();
    if voices.is_empty() {
        return None;
    }
    only(credits.iter().filter(|c| {
        c.credit_type == CreditType::Actor
            && std::iter::once(c.person_name.as_str())
                .chain(people.of(c).and_then(|p| p.original_name.as_deref()))
                .flat_map(keys)
                .any(|key| voices.contains(&key))
    }))
}

/// The id of the one credit there is.
fn only<'a>(mut credits: impl Iterator<Item = &'a Credit>) -> Option<String> {
    match (credits.next(), credits.next()) {
        (Some(credit), None) => Some(credit.id.clone()),
        _ => None,
    }
}

/// A part as a credit names it, without the note after it in brackets:
/// `Eren Yeager (voice)` is `Eren Yeager`.
fn without_note(part: &str) -> &str {
    let part = part.trim();
    match part.strip_suffix(')').and_then(|rest| rest.rfind(" (")) {
        Some(open) => part[..open].trim_end(),
        None => part,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{
            MediaKind,
            person::{CharacterRole, CharacterVoice, PersonRef},
        },
        merge::{Contribution, combine},
    };

    fn credit(name: &str, tmdb: Option<i64>, part: Option<&str>) -> Credit {
        Credit {
            id: crate::db::new_id(),
            credit_type: CreditType::Actor,
            person_name: name.into(),
            character_name: part.map(str::to_string),
            image: None,
            tmdb_person_id: tmdb,
            credit_tmdb_id: None,
            sort_order: 0,
            is_manual: false,
            department: None,
            job: None,
        }
    }

    fn character(name: &str, source: &str, order: i32) -> CharacterCredit {
        CharacterCredit {
            character: Character {
                id: String::new(),
                name: name.into(),
                native_name: None,
                image: None,
                tvdb_id: None,
                anilist_id: None,
                mal_id: None,
            },
            role: None,
            sort_order: order,
            credit_id: None,
            voices: Vec::new(),
            source: Some(source.into()),
        }
    }

    fn voice(language: &str, name: &str, native: Option<&str>) -> CharacterVoice {
        CharacterVoice {
            language: language.into(),
            name: name.into(),
            native_name: native.map(str::to_string),
            image: None,
            anilist_id: Some(1),
            mal_id: None,
            sort_order: 0,
        }
    }

    fn priority() -> Vec<String> {
        ["tmdb", "tvdb", "anilist"].map(String::from).to_vec()
    }

    /// An anime, as its three sources give it: TMDB's cast, each part named
    /// as TMDB names a voice's; TheTVDB's parts, by their records and its
    /// people; AniList's characters, by its ids, with every dub's voices.
    fn anime() -> Vec<Contribution> {
        let mut tmdb = MediaItem::empty(MediaKind::Series);
        tmdb.title = "Attack on Titan".into();
        tmdb.original_language = Some("jpn".into());
        tmdb.credits = vec![
            credit("Yuki Kaji", Some(1), Some("Eren Yeager (voice)")),
            credit("Yui Ishikawa", Some(2), Some("Mikasa Ackerman (voice)")),
            credit("Hiroshi Kamiya", Some(3), Some("Levi (voice)")),
        ];
        tmdb.people.by_credit.insert(
            tmdb.credits[0].id.clone(),
            PersonRef {
                original_name: Some("梶裕貴".into()),
                ..PersonRef::default()
            },
        );

        let mut tvdb = MediaItem::empty(MediaKind::Series);
        tvdb.title = "Attack on Titan".into();
        let kaji = credit("Kaji Yuki", None, Some("Eren Jaeger"));
        let ishikawa = credit("Yui Ishikawa", None, Some("Mikasa Ackerman"));
        for (at, credit) in [&kaji, &ishikawa].into_iter().enumerate() {
            tvdb.people.by_credit.insert(
                credit.id.clone(),
                PersonRef {
                    tvdb_id: Some(100 + at as i64),
                    ..PersonRef::default()
                },
            );
        }
        let mut eren = character("Eren Jaeger", "tvdb", 0);
        eren.character.tvdb_id = Some(7001);
        eren.character.image = Some("https://artworks.thetvdb.com/kaji.jpg".into());
        eren.role = Some(CharacterRole::Supporting);
        eren.credit_id = Some(kaji.id.clone());
        let mut mikasa = character("Mikasa Ackerman", "tvdb", 1);
        mikasa.character.tvdb_id = Some(7002);
        mikasa.credit_id = Some(ishikawa.id.clone());
        tvdb.credits = vec![kaji, ishikawa];
        tvdb.characters = vec![eren, mikasa];

        let mut anilist = MediaItem::empty(MediaKind::Series);
        anilist.title = "Attack on Titan".into();
        let mut eren = character("Eren Yeager", "anilist", 0);
        eren.character.anilist_id = Some(40882);
        eren.character.native_name = Some("エレン・イェーガー".into());
        eren.character.image = Some("https://s4.anilist.co/eren.png".into());
        eren.role = Some(CharacterRole::Main);
        eren.voices = vec![
            voice("jpn", "Yuuki Kaji", Some("梶裕貴")),
            voice("eng", "Bryce Papenbrook", None),
        ];
        let mut levi = character("Levi", "anilist", 1);
        levi.character.anilist_id = Some(45627);
        levi.role = Some(CharacterRole::Supporting);
        levi.voices = vec![voice("jpn", "Hiroshi Kamiya", Some("神谷浩史"))];
        let mut hange = character("Hange Zoë", "anilist", 2);
        hange.character.anilist_id = Some(46493);
        hange.voices = vec![voice("jpn", "Romi Park", Some("朴璐美"))];
        anilist.characters = vec![eren, levi, hange];

        [("tmdb", tmdb), ("tvdb", tvdb), ("anilist", anilist)]
            .into_iter()
            .map(|(provider, item)| Contribution {
                provider: provider.into(),
                item,
            })
            .collect()
    }

    #[test]
    fn every_sources_characters_are_kept_one_character_once_played_by_the_cast() {
        let merged = combine(anime(), &priority()).expect("merged");
        let by_credit = |id: Option<&String>| {
            id.and_then(|id| merged.credits.iter().find(|c| c.id == *id))
                .map(|c| c.person_name.as_str())
        };
        // Its name, TheTVDB's and AniList's ids, who plays it.
        type Read<'a> = (&'a str, Option<i64>, Option<i64>, Option<&'a str>);
        let read: Vec<Read<'_>> = merged
            .characters
            .iter()
            .map(|c| {
                (
                    c.character.name.as_str(),
                    c.character.tvdb_id,
                    c.character.anilist_id,
                    by_credit(c.credit_id.as_ref()),
                )
            })
            .collect();
        assert_eq!(
            read,
            [
                // One by who voices both: AniList's name, TheTVDB's place.
                ("Eren Yeager", Some(7001), Some(40882), Some("Yuki Kaji")),
                ("Mikasa Ackerman", Some(7002), None, Some("Yui Ishikawa")),
                // AniList's alone: played by the credit that names the part,
                // and nobody where none does.
                ("Levi", None, Some(45627), Some("Hiroshi Kamiya")),
                ("Hange Zoë", None, Some(46493), None),
            ]
        );

        let eren = &merged.characters[0];
        assert_eq!(
            eren.character.image.as_deref(),
            Some("https://s4.anilist.co/eren.png"),
            "AniList's drawing before TheTVDB's photograph"
        );
        assert_eq!(eren.role, Some(CharacterRole::Main), "AniList's word");
        assert_eq!(
            eren.character.native_name.as_deref(),
            Some("エレン・イェーガー")
        );
        assert_eq!(eren.source.as_deref(), Some("tvdb"));
        assert_eq!(
            eren.voices
                .iter()
                .map(|v| v.language.as_str())
                .collect::<Vec<_>>(),
            ["jpn", "eng"]
        );
        assert_eq!(
            merged
                .characters
                .iter()
                .map(|c| c.sort_order)
                .collect::<Vec<_>>(),
            [0, 1, 3, 4],
            "AniList's after TheTVDB's, in their order"
        );
    }

    #[test]
    fn a_part_is_played_by_the_one_credit_that_names_it_or_voices_it() {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.original_language = Some("jpn".into());
        item.credits = vec![
            credit("Somebody", Some(1), Some("Alpha / Beta (voice)")),
            credit("Twice", Some(2), Some("Gamma")),
            credit("Again", Some(3), Some("Gamma")),
            credit("Atsumi Tanezaki", Some(4), Some("Somebody Else")),
        ];
        let beta = character("beta", "anilist", 0);
        let gamma = character("Gamma", "anilist", 1);
        let mut frieren = character("Frieren", "anilist", 2);
        frieren.voices = vec![
            voice("eng", "Mallorie Rodak", None),
            voice("jpn", "Atsumi Tanezaki", Some("種﨑敦美")),
        ];
        let mut english = character("Somebody English", "anilist", 3);
        english.voices = vec![voice("eng", "Atsumi Tanezaki", None)];
        item.characters = vec![beta, gamma, frieren, english];

        cast(&mut item);
        let played: Vec<Option<&str>> = item
            .characters
            .iter()
            .map(|c| {
                c.credit_id
                    .as_ref()
                    .and_then(|id| item.credits.iter().find(|cr| cr.id == *id))
                    .map(|cr| cr.person_name.as_str())
            })
            .collect();
        assert_eq!(
            played,
            [Some("Somebody"), None, Some("Atsumi Tanezaki"), None],
            "one of two parts, a note aside; never one of two credits; the \
             original voice, not a dub's"
        );
    }

    #[test]
    fn two_characters_are_one_only_when_nothing_says_otherwise() {
        let mut ours = MediaItem::empty(MediaKind::Series);
        ours.original_language = Some("jpn".into());
        let voiced = credit("Rie Kugimiya", Some(9), None);
        let mut alphonse = character("Alphonse Elric", "tvdb", 0);
        alphonse.character.tvdb_id = Some(1);
        alphonse.credit_id = Some(voiced.id.clone());
        let mut guard = character("Guard", "tvdb", 1);
        guard.character.tvdb_id = Some(2);
        ours.credits = vec![voiced];
        ours.characters = vec![alphonse, guard];

        // TheTVDB's other record of a guard is another guard; one person
        // voicing two of AniList's is nobody's to pair by.
        let mut other_guard = character("Guard", "tvdb", 0);
        other_guard.character.tvdb_id = Some(3);
        let mut al = character("Al", "anilist", 1);
        al.voices = vec![voice("jpn", "Rie Kugimiya", None)];
        let mut nina = character("Nina", "anilist", 2);
        nina.voices = vec![voice("jpn", "Rie Kugimiya", None)];
        union(&mut ours, vec![other_guard, al, nina], &BTreeMap::new());

        assert_eq!(
            ours.characters
                .iter()
                .map(|c| c.character.name.as_str())
                .collect::<Vec<_>>(),
            ["Alphonse Elric", "Guard", "Guard", "Al", "Nina"]
        );

        // Voiced alone by them, Al is Alphonse.
        let mut ours = MediaItem::empty(MediaKind::Series);
        ours.original_language = Some("jpn".into());
        let voiced = credit("Rie Kugimiya", Some(9), None);
        let mut alphonse = character("Alphonse Elric", "tvdb", 0);
        alphonse.credit_id = Some(voiced.id.clone());
        ours.credits = vec![voiced];
        ours.characters = vec![alphonse];
        let mut al = character("Al", "anilist", 0);
        al.character.anilist_id = Some(12);
        al.voices = vec![voice("jpn", "Kugimiya Rie", None)];
        union(&mut ours, vec![al], &BTreeMap::new());
        assert_eq!(ours.characters.len(), 1);
        assert_eq!(ours.characters[0].character.anilist_id, Some(12));
    }

    #[test]
    fn a_credit_that_went_into_another_takes_its_characters_with_it() {
        let mut ours = MediaItem::empty(MediaKind::Series);
        let kept = credit("Bryan Cranston", Some(17419), Some("Walter White"));
        ours.credits = vec![kept.clone()];
        let theirs_credit = credit("Bryan Cranston", None, Some("Walter White"));
        let mut walter = character("Walter White", "tvdb", 0);
        walter.character.tvdb_id = Some(64178858);
        walter.credit_id = Some(theirs_credit.id.clone());
        let mut gone = character("Nobody's Part", "tvdb", 1);
        gone.character.tvdb_id = Some(1);
        gone.credit_id = Some("no such credit".into());

        let renamed = BTreeMap::from([(theirs_credit.id.clone(), kept.id.clone())]);
        union(&mut ours, vec![walter, gone], &renamed);
        assert_eq!(
            ours.characters[0].credit_id.as_deref(),
            Some(kept.id.as_str())
        );
        assert_eq!(ours.characters[1].credit_id, None);
    }

    #[test]
    fn a_note_after_a_part_is_no_part_of_its_name() {
        assert_eq!(without_note("Eren Yeager (voice)"), "Eren Yeager");
        assert_eq!(without_note(" Narrator "), "Narrator");
        assert_eq!(without_note("(voice)"), "(voice)");
        assert_eq!(keys("Yeager  Eren")[0], keys("eren yeager")[0]);
        assert_eq!(keys("梶 裕貴")[1], keys("梶裕貴")[0]);
    }
}
