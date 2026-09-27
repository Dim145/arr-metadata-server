//! What each source gives a work, and which wins: the merge's rules, laid out
//! for the people who have to trust what it made.
//!
//! Declared rather than worked out, because what a provider *can* give is a
//! property of its mapping, not of the works this server happens to hold. The
//! order is not declared: it is the configured priority, so the table always
//! says what the next merge will do.

use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    domain::MediaKind,
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
        Whole,
        &[TMDB, RADARR, ANILIST, FANKAI],
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
    line(People, "credits", Whole, &[TMDB, SKYHOOK, RADARR, FANKAI]),
    with_authorities(
        line(People, "studio", First, &[TMDB, RADARR, ANILIST, MAL]),
        STUDIO_AUTHORITIES,
    ),
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
