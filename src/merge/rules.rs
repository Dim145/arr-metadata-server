//! What each source gives a work, and which wins: the merge's rules, laid out
//! for the people who have to trust what it made.
//!
//! Declared rather than worked out, because what a provider *can* give is a
//! property of its mapping, not of the works this server happens to hold. The
//! order is not declared: it is the `sources.priority` setting, which the merge
//! reads each time it runs, so the table always says what the next merge will
//! do.

use std::collections::BTreeMap;

use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    domain::{
        Credit, MediaKind,
        person::{People, PersonRef},
    },
    providers::names::{
        ANILIST, FANART, FANKAI, FANKAI_WIKI, MAL, RADARR, SKYHOOK, TMDB, TVDB, TVMAZE,
    },
};

use super::{DATE_CHECKED, STUDIO_AUTHORITIES, TVDB_NUMBERED};

/// Every provider a work can be made from, IMDb aside: its rating is laid
/// over at reading time and it never contributes to a merge.
pub const PROVIDERS: &[&str] = &[
    TMDB,
    TVDB,
    SKYHOOK,
    RADARR,
    FANART,
    TVMAZE,
    ANILIST,
    MAL,
    FANKAI,
    FANKAI_WIKI,
];

/// How a row's value is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum Rule {
    /// The first, in priority order, that has one gives it.
    First,
    /// A list taken whole from the first that has one: nothing tells the same
    /// actor or genre apart across providers well enough to blend them.
    Whole,
    /// Every provider adds its own.
    Union,
    /// Whoever numbers the episodes — TheTVDB, or Skyhook — first; the rest
    /// fill in, episode by episode, in priority order.
    Spine,
    /// True if any says so.
    Either,
}

/// What a row is about, for grouping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum Group {
    Identity,
    Release,
    Classification,
    People,
    Episodes,
}

struct Line {
    group: Group,
    /// The key the page names the row by.
    field: &'static str,
    rule: Rule,
    /// Who can supply it. The order here does not matter: the priority does.
    from: &'static [&'static str],
    /// Whose value replaces every other's, when anyone's does.
    authorities: &'static [&'static str],
    only: Option<MediaKind>,
}

const fn line(
    group: Group,
    field: &'static str,
    rule: Rule,
    from: &'static [&'static str],
) -> Line {
    Line {
        group,
        field,
        rule,
        from,
        authorities: &[],
        only: None,
    }
}

const fn only(mut line: Line, kind: MediaKind) -> Line {
    line.only = Some(kind);
    line
}

const fn with_authorities(mut line: Line, authorities: &'static [&'static str]) -> Line {
    line.authorities = authorities;
    line
}

use Group::*;
use Rule::*;

/// Read off the mappers, one provider at a time. No mapper gives a sort
/// title: only a person does.
const LINES: &[Line] = &[
    // Identity
    line(
        Identity,
        "title",
        First,
        &[TMDB, TVDB, SKYHOOK, RADARR, TVMAZE, ANILIST, MAL, FANKAI],
    ),
    line(
        Identity,
        "originalTitle",
        First,
        &[TMDB, RADARR, ANILIST, MAL, FANKAI],
    ),
    line(Identity, "sortTitle", First, &[]),
    line(
        Identity,
        "overview",
        First,
        &[TMDB, TVDB, SKYHOOK, RADARR, TVMAZE, ANILIST, MAL, FANKAI],
    ),
    line(
        Identity,
        "status",
        First,
        &[TMDB, TVDB, SKYHOOK, RADARR, FANKAI],
    ),
    line(
        Identity,
        "year",
        First,
        &[TMDB, TVDB, SKYHOOK, RADARR, ANILIST, MAL, FANKAI],
    ),
    line(
        Identity,
        "originalLanguage",
        First,
        &[TMDB, TVDB, SKYHOOK, RADARR, FANKAI],
    ),
    line(
        Identity,
        "originalCountry",
        First,
        &[TMDB, TVDB, SKYHOOK, FANKAI],
    ),
    line(
        Identity,
        "alternativeTitles",
        Union,
        &[TMDB, TVDB, SKYHOOK, RADARR, ANILIST, MAL, FANKAI],
    ),
    line(Identity, "translations", Union, &[TMDB, TVDB, RADARR]),
    line(Identity, "tagline", First, &[TMDB]),
    line(Identity, "productionCountries", Whole, &[TMDB]),
    // Airing and release
    only(
        line(Release, "aired", First, &[TMDB, TVDB, SKYHOOK, FANKAI]),
        MediaKind::Series,
    ),
    only(
        line(Release, "releases", First, &[TMDB, RADARR]),
        MediaKind::Movie,
    ),
    only(
        line(Release, "airTime", First, &[TVDB, SKYHOOK]),
        MediaKind::Series,
    ),
    only(
        line(Release, "network", First, &[TMDB, TVDB, SKYHOOK, FANKAI]),
        MediaKind::Series,
    ),
    line(
        Release,
        "runtime",
        First,
        &[TMDB, TVDB, SKYHOOK, RADARR, TVMAZE, FANKAI],
    ),
    only(
        line(Release, "collection", First, &[TMDB]),
        MediaKind::Movie,
    ),
    // Classification
    line(
        Classification,
        "genres",
        Whole,
        &[TMDB, TVDB, SKYHOOK, RADARR, ANILIST, MAL, FANKAI],
    ),
    line(
        Classification,
        "keywords",
        Union,
        &[TMDB, RADARR, ANILIST, FANKAI],
    ),
    line(Classification, "format", First, &[ANILIST]),
    line(
        Classification,
        "countryReleases",
        Union,
        &[TMDB, TVDB, RADARR],
    ),
    line(
        Classification,
        "contentRating",
        First,
        &[TMDB, TVDB, SKYHOOK, RADARR],
    ),
    line(
        Classification,
        "ratings",
        Union,
        &[TMDB, SKYHOOK, RADARR, ANILIST, MAL, FANKAI],
    ),
    line(Classification, "isAdult", Either, &[TMDB, ANILIST, MAL]),
    // People and pictures
    line(
        People,
        "credits",
        Whole,
        &[TMDB, TVDB, SKYHOOK, RADARR, FANKAI],
    ),
    // TheTVDB's characters are matched into the list, whoever gave it: see
    // `merge_cast`.
    line(People, "castMatched", Union, &[TVDB]),
    only(
        line(People, "episodeCredits", First, &[TMDB]),
        MediaKind::Series,
    ),
    with_authorities(
        line(People, "studio", First, &[TMDB, RADARR, ANILIST, MAL]),
        STUDIO_AUTHORITIES,
    ),
    line(People, "productionCompanies", Whole, &[TMDB]),
    line(
        People,
        "images",
        Union,
        &[TMDB, TVDB, SKYHOOK, RADARR, FANART, ANILIST, MAL, FANKAI],
    ),
    line(People, "themeMusic", First, &[FANKAI]),
    line(People, "trailer", First, &[TMDB, RADARR]),
    line(People, "homepage", First, &[TMDB, RADARR, FANKAI_WIKI]),
    line(People, "relations", Whole, &[ANILIST, FANKAI_WIKI]),
    // Episodes. TVmaze never supplies the list nor its titles: it numbers some
    // series its own way, so it only fills gaps on days with one episode.
    only(
        line(
            Episodes,
            "episodeList",
            Spine,
            &[TVDB, SKYHOOK, TMDB, FANKAI],
        ),
        MediaKind::Series,
    ),
    only(
        line(
            Episodes,
            "episodeText",
            Spine,
            &[TVDB, SKYHOOK, TMDB, TVMAZE, FANKAI],
        ),
        MediaKind::Series,
    ),
    only(
        line(
            Episodes,
            "episodeStills",
            Spine,
            &[TVDB, SKYHOOK, TMDB, TVMAZE, FANKAI],
        ),
        MediaKind::Series,
    ),
    only(
        with_authorities(
            line(Episodes, "airDateUtc", Spine, &[SKYHOOK, TVMAZE]),
            DATE_CHECKED,
        ),
        MediaKind::Series,
    ),
    only(
        line(
            Episodes,
            "absoluteNumbering",
            Spine,
            &[TVDB, SKYHOOK, FANKAI],
        ),
        MediaKind::Series,
    ),
];

/// The cast and crew as the merge makes them, one provider's into another's
/// (`fold`): the list of the first that has one, whole, as before — nothing
/// tells two providers' people apart well enough to blend two lists — with
/// TheTVDB's characters matched into it, whoever gave it.
///
/// TheTVDB's credits are told by the people id they carry (`people`) where
/// they name no TMDB id. They stand for nobody's cast: when they are all the
/// list holds — TheTVDB ahead of the others in the priority — the next list
/// that is not theirs is the list, and they are matched into it. Each is
/// matched to the list's credit of the same kind for the same person — by
/// TMDB's id where both give one, else by name, accents, case and the order
/// of its words aside, or by the name the list's credit gives in its own
/// script — and completes it: TheTVDB's id of the person, a character or a
/// portrait it had none of. Unmatched, it is added after the list, in
/// TheTVDB's order — a part TMDB's cast was cut short of, a series TMDB does
/// not list the cast of — given the TMDB id of the guest of an episode it is,
/// where one is. Everybody else's credits are dropped, as before.
///
/// The credits of single episodes are the first provider's to have any.
pub fn merge_cast(
    credits: &mut Vec<Credit>,
    people: &mut People,
    other_credits: Vec<Credit>,
    other: People,
) {
    let People {
        episodes,
        by_credit: mut known,
    } = other;
    if people.episodes.is_empty() && !episodes.is_empty() {
        for given in &episodes {
            if let Some(person) = known.remove(&given.credit.id) {
                people.by_credit.insert(given.credit.id.clone(), person);
            }
        }
        people.episodes = episodes;
    }

    let (theirs, characters): (Vec<Credit>, Vec<Credit>) = other_credits
        .into_iter()
        .partition(|c| !is_character(c, &known));
    let ours_are_characters =
        !credits.is_empty() && credits.iter().all(|c| is_character(c, &people.by_credit));

    if credits.is_empty() || (ours_are_characters && !theirs.is_empty()) {
        // Their list is the list; TheTVDB's characters held so far are
        // matched into it with theirs.
        let held = std::mem::take(credits);
        let mut held_known = BTreeMap::new();
        for credit in &held {
            if let Some(person) = people.by_credit.remove(&credit.id) {
                held_known.insert(credit.id.clone(), person);
            }
        }
        for credit in &theirs {
            if let Some(person) = known.remove(&credit.id) {
                people.by_credit.insert(credit.id.clone(), person);
            }
        }
        *credits = theirs;
        match_in(credits, people, held, held_known);
    }
    match_in(credits, people, characters, known);
}

/// Whether a credit is one of TheTVDB's characters: a TheTVDB people id, and
/// no TMDB id.
fn is_character(credit: &Credit, known: &BTreeMap<String, PersonRef>) -> bool {
    credit.tmdb_person_id.is_none() && known.get(&credit.id).is_some_and(|p| p.tvdb_id.is_some())
}

/// Match TheTVDB's characters into the list, adding those it lacks after it.
fn match_in(
    credits: &mut Vec<Credit>,
    people: &mut People,
    characters: Vec<Credit>,
    mut known: BTreeMap<String, PersonRef>,
) {
    let mut next = credits
        .iter()
        .map(|c| c.sort_order)
        .max()
        .map_or(0, |n| n.saturating_add(1));
    for mut theirs in characters {
        let Some(their_person) = known.remove(&theirs.id) else {
            continue;
        };
        let matched = credits.iter_mut().find(|ours| {
            ours.credit_type == theirs.credit_type
                && same_person(ours, people.of(ours), &theirs, Some(&their_person))
        });
        match matched {
            Some(ours) => {
                let person = people.by_credit.entry(ours.id.clone()).or_default();
                person.tvdb_id = person.tvdb_id.or(their_person.tvdb_id);
                if ours.character_name.is_none() {
                    ours.character_name = theirs.character_name;
                }
                if ours.image.is_none() {
                    ours.image = theirs.image;
                }
            }
            None => {
                // A guest of an episode TMDB names is the same person by
                // the same name on the same series.
                if theirs.tmdb_person_id.is_none()
                    && let Some(guest) = people.episodes.iter().find(|e| {
                        e.credit.tmdb_person_id.is_some()
                            && same_person(
                                &e.credit,
                                people.of(&e.credit),
                                &theirs,
                                Some(&their_person),
                            )
                    })
                {
                    theirs.tmdb_person_id = guest.credit.tmdb_person_id;
                }
                theirs.sort_order = next;
                next = next.saturating_add(1);
                people.by_credit.insert(theirs.id.clone(), their_person);
                credits.push(theirs);
            }
        }
    }
}

/// Whether two providers' credits name one person: the same TMDB id where
/// both give one; else the same name, its accents, its case and the order
/// of its words aside — TheTVDB writes some Japanese names family name
/// first where TMDB does not — or the name one gives in its own script
/// being the other's.
fn same_person(
    ours: &Credit,
    our_person: Option<&PersonRef>,
    theirs: &Credit,
    their_person: Option<&PersonRef>,
) -> bool {
    if let (Some(a), Some(b)) = (ours.tmdb_person_id, theirs.tmdb_person_id) {
        return a == b;
    }
    let words = |name: &str| {
        let mut words: Vec<String> = crate::search::norm(name)
            .split_whitespace()
            .map(str::to_string)
            .collect();
        words.sort_unstable();
        words
    };
    let theirs_named = words(&theirs.person_name);
    if theirs_named.is_empty() {
        return false;
    }
    std::iter::once(ours.person_name.as_str())
        .chain(our_person.and_then(|p| p.original_name.as_deref()))
        .any(|name| words(name) == theirs_named)
        || their_person
            .and_then(|p| p.original_name.as_deref())
            .is_some_and(|name| words(name) == words(&ours.person_name))
}

/// A row as the page draws it.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Laid {
    pub group: Group,
    pub field: &'static str,
    pub rule: Rule,
    /// Who supplies it, in the order their values are taken.
    pub suppliers: Vec<&'static str>,
    /// Whose value replaces every other's.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authorities: Vec<&'static str>,
    /// When it applies to one kind of work only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only: Option<MediaKind>,
}

/// A priority as the `sources.priority` setting holds it — providers by name,
/// most trusted first, separated by commas — read and checked: each a name
/// the merge knows, none twice, at least one. Whatever it leaves out comes
/// after, as [`order`] puts it.
pub fn parse_priority(text: &str) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = Vec::new();
    for name in text
        .split(',')
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
    {
        if !PROVIDERS.contains(&name.as_str()) {
            return Err(format!(
                "names no provider called {name:?}: the providers are {}",
                PROVIDERS.join(", ")
            ));
        }
        if names.contains(&name) {
            return Err(format!("names {name} twice"));
        }
        names.push(name);
    }
    if names.is_empty() {
        return Err("names no provider".into());
    }
    Ok(names)
}

/// Every provider in the order the merge consults them: the configured
/// priority, then whatever it leaves out, as the merge sorts those.
pub fn order(priority: &[String]) -> Vec<&'static str> {
    let mut providers = PROVIDERS.to_vec();
    let rank = |p: &str| {
        priority
            .iter()
            .position(|q| q == p)
            .unwrap_or(priority.len())
    };
    providers.sort_by_key(|p| rank(p));
    providers
}

/// The table, its suppliers in the order the next merge will take them.
pub fn laid_out(priority: &[String]) -> Vec<Laid> {
    let order = order(priority);
    let at = |p: &str| order.iter().position(|q| *q == p).unwrap_or(order.len());

    LINES
        .iter()
        .map(|line| {
            let mut suppliers: Vec<&'static str> = line
                .from
                .iter()
                .copied()
                .filter(|p| !line.authorities.contains(p))
                .collect();
            // The numbering's own providers lead whatever the priority says:
            // a list numbered any other way would have Sonarr file episodes
            // under the wrong numbers.
            suppliers.sort_by_key(|p| {
                let leads = line.rule == Rule::Spine && TVDB_NUMBERED.contains(p);
                (!leads, at(p))
            });

            Laid {
                group: line.group,
                field: line.field,
                rule: line.rule,
                suppliers,
                authorities: line.authorities.to_vec(),
                only: line.only,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::CreditType;

    fn priority() -> Vec<String> {
        [
            "tmdb",
            "tvdb",
            "skyhook",
            "radarr",
            "fanart",
            "tvmaze",
            "anilist",
            "mal",
            "fankai",
            "fankaiwiki",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn the_priority_decides_the_order_and_the_numbering_leads_its_rows() {
        let rows = laid_out(&priority());
        let title = rows.iter().find(|r| r.field == "title").unwrap();
        assert_eq!(title.suppliers[..2], ["tmdb", "tvdb"]);

        let list = rows.iter().find(|r| r.field == "episodeList").unwrap();
        assert_eq!(list.suppliers[..3], ["tvdb", "skyhook", "tmdb"]);
    }

    #[test]
    fn an_authority_is_not_ranked_beside_the_rest() {
        let rows = laid_out(&priority());
        let studio = rows.iter().find(|r| r.field == "studio").unwrap();
        assert_eq!(studio.authorities, ["anilist", "mal"]);
        assert!(!studio.suppliers.contains(&"anilist"));
    }

    #[test]
    fn a_provider_the_priority_leaves_out_comes_last() {
        let order = order(&["tvdb".to_string()]);
        assert_eq!(order[0], "tvdb");
        assert_eq!(order.len(), PROVIDERS.len());
    }

    #[test]
    fn a_priority_written_as_a_setting_is_read_back_and_checked() {
        // As the Sources page writes it, and as somebody types it.
        let read = parse_priority(" TVDB, tmdb ,,skyhook").expect("read");
        assert_eq!(read, ["tvdb", "tmdb", "skyhook"]);
        assert_eq!(order(&read)[..3], ["tvdb", "tmdb", "skyhook"]);
        assert_eq!(order(&read).len(), PROVIDERS.len());

        let everyone = PROVIDERS.join(",");
        assert_eq!(
            parse_priority(&everyone).expect("read").len(),
            PROVIDERS.len()
        );

        // A name the merge does not know, one named twice, or none at all.
        assert!(parse_priority("tmdb,imdb").unwrap_err().contains("imdb"));
        assert!(
            parse_priority("tmdb,tvdb,tmdb")
                .unwrap_err()
                .contains("twice")
        );
        assert!(parse_priority(" , ").is_err());
    }

    fn credit(kind: CreditType, name: &str, tmdb: Option<i64>) -> Credit {
        Credit {
            id: crate::db::new_id(),
            credit_type: kind,
            person_name: name.into(),
            character_name: None,
            image: None,
            tmdb_person_id: tmdb,
            credit_tmdb_id: None,
            sort_order: 0,
            is_manual: false,
        }
    }

    /// TheTVDB's credit of somebody, with its id of them.
    fn tvdb(name: &str, id: i64, character: &str, people: &mut People) -> Credit {
        let mut c = credit(CreditType::Actor, name, None);
        c.character_name = Some(character.into());
        c.image = Some(format!("https://tvdb/{id}.jpg"));
        people.by_credit.insert(
            c.id.clone(),
            PersonRef {
                tvdb_id: Some(id),
                ..PersonRef::default()
            },
        );
        c
    }

    #[test]
    fn thetvdb_s_people_are_matched_into_the_cast_and_the_others_added_after_it() {
        let mut ours = vec![
            credit(CreditType::Actor, "Bryan Cranston", Some(17419)),
            credit(CreditType::Actor, "Mayumi Tanaka", Some(1)),
            credit(CreditType::Director, "Vince Gilligan", Some(66633)),
        ];
        ours[1].sort_order = 7;
        let mut people = People::default();
        people.by_credit.insert(
            ours[1].id.clone(),
            PersonRef {
                original_name: Some("田中真弓".into()),
                ..PersonRef::default()
            },
        );

        let mut theirs_people = People::default();
        let theirs = vec![
            tvdb("bryan cranston", 10, "Walter White", &mut theirs_people),
            tvdb("Tanaka Mayumi", 20, "Luffy", &mut theirs_people),
            tvdb("田中真弓", 21, "Luffy", &mut theirs_people),
            tvdb("Aaron Paul", 30, "Jesse Pinkman", &mut theirs_people),
            // Skyhook's, say, with no TheTVDB id: one list, not a blend.
            credit(CreditType::Actor, "Someone Else", None),
        ];
        merge_cast(&mut ours, &mut people, theirs, theirs_people);

        let read: Vec<(&str, Option<&str>, i32)> = ours
            .iter()
            .map(|c| {
                (
                    c.person_name.as_str(),
                    c.character_name.as_deref(),
                    c.sort_order,
                )
            })
            .collect();
        assert_eq!(
            read,
            [
                ("Bryan Cranston", Some("Walter White"), 0),
                ("Mayumi Tanaka", Some("Luffy"), 7),
                ("Vince Gilligan", None, 0),
                ("Aaron Paul", Some("Jesse Pinkman"), 8),
            ],
            "matched whatever the case and the order of the words, or by the name in \
             its own script; the rest after"
        );
        assert_eq!(people.of(&ours[0]).and_then(|p| p.tvdb_id), Some(10));
        assert_eq!(people.of(&ours[1]).and_then(|p| p.tvdb_id), Some(20));
        assert_eq!(
            people.of(&ours[1]).and_then(|p| p.original_name.as_deref()),
            Some("田中真弓"),
            "what TMDB said of her stays"
        );
        assert_eq!(people.of(&ours[3]).and_then(|p| p.tvdb_id), Some(30));
        assert_eq!(ours[0].image.as_deref(), Some("https://tvdb/10.jpg"));
    }

    #[test]
    fn a_list_nobody_gave_is_the_next_one_s_and_so_are_its_episodes() {
        let mut ours: Vec<Credit> = Vec::new();
        let mut people = People::default();
        let mut theirs_people = People::default();
        let theirs = vec![tvdb(
            "Anna Gunn",
            269404,
            "Skyler White",
            &mut theirs_people,
        )];
        let guest = credit(CreditType::Guest, "A Guest", Some(5));
        theirs_people.episodes = vec![crate::domain::person::EpisodeCredit {
            season_number: 1,
            episode_number: 2,
            tmdb_episode_id: Some(62086),
            credit: guest.clone(),
        }];
        theirs_people.by_credit.insert(
            guest.id.clone(),
            PersonRef {
                gender: Some(2),
                ..PersonRef::default()
            },
        );
        merge_cast(&mut ours, &mut people, theirs, theirs_people);
        assert_eq!(ours.len(), 1);
        assert_eq!(people.of(&ours[0]).and_then(|p| p.tvdb_id), Some(269404));
        assert_eq!(people.episodes.len(), 1);
        assert_eq!(people.of(&guest).and_then(|p| p.gender), Some(2));

        // A later provider's episodes do not replace the first's.
        let later = People {
            episodes: vec![crate::domain::person::EpisodeCredit {
                season_number: 9,
                episode_number: 9,
                tmdb_episode_id: None,
                credit: credit(CreditType::Guest, "Late", None),
            }],
            ..People::default()
        };
        merge_cast(&mut ours, &mut people, Vec::new(), later);
        assert_eq!(people.episodes[0].season_number, 1);
    }

    #[test]
    fn thetvdb_ahead_of_tmdb_gives_no_list_of_its_own_and_a_guest_s_id_is_taken() {
        // TheTVDB first in the priority: its characters are the list only
        // until a list that is not theirs comes.
        let mut ours_people = People::default();
        let mut ours = vec![
            tvdb("Bryan Cranston", 10, "Walter White", &mut ours_people),
            tvdb("Giancarlo Esposito", 40, "Gus Fring", &mut ours_people),
        ];
        let guest = credit(CreditType::Guest, "Giancarlo Esposito", Some(4808));
        let tmdb = People {
            episodes: vec![crate::domain::person::EpisodeCredit {
                season_number: 2,
                episode_number: 11,
                tmdb_episode_id: Some(62171),
                credit: guest,
            }],
            ..People::default()
        };
        let theirs = vec![
            credit(CreditType::Actor, "Bryan Cranston", Some(17419)),
            credit(CreditType::Actor, "Aaron Paul", Some(84497)),
        ];
        merge_cast(&mut ours, &mut ours_people, theirs, tmdb);

        let read: Vec<(&str, Option<i64>)> = ours
            .iter()
            .map(|c| (c.person_name.as_str(), c.tmdb_person_id))
            .collect();
        assert_eq!(
            read,
            [
                ("Bryan Cranston", Some(17419)),
                ("Aaron Paul", Some(84497)),
                ("Giancarlo Esposito", Some(4808)),
            ],
            "TMDB's list, TheTVDB's character matched into it, the one it lacks after it \
             with the TMDB id the guest of the same name has"
        );
        assert_eq!(ours_people.of(&ours[0]).and_then(|p| p.tvdb_id), Some(10));
        assert_eq!(ours_people.of(&ours[2]).and_then(|p| p.tvdb_id), Some(40));
        assert_eq!(ours[0].character_name.as_deref(), Some("Walter White"));
        assert_eq!(ours_people.episodes.len(), 1);

        // Skyhook's list after it is dropped, as a second list always is.
        let skyhook = vec![credit(CreditType::Actor, "Somebody Else", None)];
        merge_cast(&mut ours, &mut ours_people, skyhook, People::default());
        assert_eq!(ours.len(), 3);
    }

    #[test]
    fn every_row_names_known_providers_once() {
        for line in LINES {
            for provider in line.from.iter().chain(line.authorities) {
                assert!(
                    PROVIDERS.contains(provider),
                    "{} names {provider}",
                    line.field
                );
            }
            let mut seen = line.from.to_vec();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(
                seen.len(),
                line.from.len(),
                "{} names a provider twice",
                line.field
            );
        }
    }
}
