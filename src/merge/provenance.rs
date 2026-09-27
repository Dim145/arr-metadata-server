//! Where each of a work's values came from.
//!
//! Worked out after the merge rather than inside it: every rule the merge
//! applies — the first answer wins, a list is taken whole, an authority
//! replaces the rest — leaves the kept value equal to the value of the
//! provider it came from. So each provider's own answer is compared with what
//! was kept: the first of them, in priority order, that said the same gave
//! it, and the others agree. Nothing in the merge has to know it is being
//! watched.
//!
//! Kept whole, Skyhook beside the TheTVDB it republishes included: a sync
//! from one source hands every other its values back from this record, and a
//! copy is still a provider that gave the value. How many witnesses that makes
//! is the reader's business.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::{
    AlternativeTitle, Credit, Episode, Image, MediaItem, MediaKind, Relation, Season,
    fields::ITEM_FIELDS,
};

use super::{Contribution, DATE_CHECKED, TVDB_NUMBERED};

/// What the merge takes from the first provider that has one, beyond the
/// fields a person may edit. Never shown beside a field, but needed to hand a
/// provider back what it gave when another is synced alone.
const ALSO_TRACKED: &[&str] = &[
    "contentRatingCountry",
    "popularity",
    "collectionTmdbId",
    "credits",
    "relations",
];

/// The work as stored, beneath every provider in a sync from chosen sources:
/// whatever nobody gave anew stays as it was. Never a provider of record.
pub const STORED: &str = "stored";

/// Who gave a work its values.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    /// Field name, as the field registry names it, to who gave its value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Source>,
    /// The images kept, counted by the provider they came from.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub images: BTreeMap<String, usize>,
    /// The provider the episode list, and its numbering, came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episodes: Option<String>,
    /// Each translation kept, by language, to the provider it came from.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub translations: BTreeMap<String, String>,
    /// Each rating kept, by the agency it is filed under, to the provider that
    /// reported it: Radarr and TMDB both report TMDB's.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ratings: BTreeMap<String, String>,
    /// Each alternative title kept — by its title and language, as
    /// [`title_key`] writes them — to the provider it came from.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub alternative_titles: BTreeMap<String, String>,
}

/// An alternative title's key in [`Provenance::alternative_titles`].
pub fn title_key(title: &AlternativeTitle) -> String {
    format!(
        "{}|{}",
        title.title.trim().to_lowercase(),
        title.language.as_deref().unwrap_or_default()
    )
}

/// Who gave one value, and who else had a say.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// The provider whose value was kept.
    pub from: String,
    /// Others that gave the same value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agreed: Vec<String>,
    /// Others that gave another value, and lost to it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differed: Vec<String>,
    /// Those ahead of it in priority that gave none: why a lower source's
    /// value stands. Sources that describe nothing — artwork alone — are not
    /// counted, or every field would name them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passed: Vec<String>,
}

impl Provenance {
    /// Every provider this record says gave something.
    pub fn providers(&self) -> BTreeSet<&str> {
        let mut named = BTreeSet::new();
        for source in self.fields.values() {
            named.insert(source.from.as_str());
            named.extend(source.agreed.iter().map(String::as_str));
        }
        named.extend(self.images.keys().map(String::as_str));
        named.extend(self.episodes.as_deref());
        named.extend(self.translations.values().map(String::as_str));
        named.extend(self.ratings.values().map(String::as_str));
        named.extend(self.alternative_titles.values().map(String::as_str));

        // Pictures added by hand, or filed without a source, are nobody's to
        // give back: they stay with the stored work.
        for not_a_provider in [STORED, "manual", "unknown"] {
            named.remove(not_a_provider);
        }
        named
    }
}

/// What one provider said, field by field, in the form values are compared
/// in.
pub struct View {
    provider: String,
    values: BTreeMap<&'static str, serde_json::Value>,
    has_episodes: bool,
    /// The languages it translated into, the agencies it reported a rating
    /// for and the alternative titles it gave: the merge keeps the first
    /// provider's of each.
    languages: BTreeSet<String>,
    agencies: BTreeSet<String>,
    titles: Vec<(String, Option<String>)>,
}

/// The members attribution compares, borrowed from a work and named and
/// shaped as `MediaItem` names and shapes them. Serialising the whole work —
/// a thousand episodes, their pictures — once per provider on every refresh
/// cost more than the rest of the attribution together; a test holds the two
/// shapes together.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Tracked<'a> {
    title: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    sort_title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    overview: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_language: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_country: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_aired: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_aired: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    in_cinemas: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    physical_release: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    digital_release: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    air_time: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    network: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    studio: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_rating: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_rating_country: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    homepage: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trailer_youtube_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    theme_music: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    popularity: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    collection_tmdb_id: Option<i64>,
    genres: &'a [String],
    keywords: &'a [String],
    #[serde(skip_serializing_if = "<[Credit]>::is_empty")]
    credits: &'a [Credit],
    #[serde(skip_serializing_if = "<[Relation]>::is_empty")]
    relations: &'a [Relation],
}

fn tracked_json(item: &MediaItem) -> serde_json::Value {
    serde_json::to_value(Tracked {
        title: &item.title,
        sort_title: item.sort_title.as_deref(),
        original_title: item.original_title.as_deref(),
        overview: item.overview.as_deref(),
        status: item.status.as_deref(),
        original_language: item.original_language.as_deref(),
        original_country: item.original_country.as_deref(),
        runtime: item.runtime,
        year: item.year,
        first_aired: item.first_aired.as_deref(),
        last_aired: item.last_aired.as_deref(),
        in_cinemas: item.in_cinemas.as_deref(),
        physical_release: item.physical_release.as_deref(),
        digital_release: item.digital_release.as_deref(),
        air_time: item.air_time.as_deref(),
        network: item.network.as_deref(),
        studio: item.studio.as_deref(),
        content_rating: item.content_rating.as_deref(),
        content_rating_country: item.content_rating_country.as_deref(),
        homepage: item.homepage.as_deref(),
        trailer_youtube_id: item.trailer_youtube_id.as_deref(),
        theme_music: item.theme_music.as_deref(),
        popularity: item.popularity,
        collection_tmdb_id: item.collection_tmdb_id,
        genres: &item.genres,
        keywords: &item.keywords,
        credits: &item.credits,
        relations: &item.relations,
    })
    .unwrap_or_default()
}

fn tracked() -> impl Iterator<Item = &'static str> {
    ITEM_FIELDS
        .iter()
        .map(|def| def.name)
        .chain(ALSO_TRACKED.iter().copied())
}

/// A provider's answer, taken apart for comparing.
pub fn view(provider: &str, item: &MediaItem) -> View {
    let json = tracked_json(item);
    let values = tracked()
        .filter_map(|name| {
            let value = json.get(name)?;
            (!is_empty(value)).then(|| (name, normalized(value)))
        })
        .collect();

    View {
        provider: provider.to_string(),
        values,
        has_episodes: !item.episodes.is_empty(),
        languages: item
            .translations
            .iter()
            .map(|t| t.language.clone())
            .collect(),
        agencies: item.ratings.iter().map(|r| r.source.clone()).collect(),
        titles: item
            .alternative_titles
            .iter()
            .map(|t| (t.title.clone(), t.language.clone()))
            .collect(),
    }
}

/// Who gave the merged work its values. `views` are in priority order, the
/// order the merge folded them in.
pub fn attribute(merged: &MediaItem, views: &[View]) -> Provenance {
    let json = tracked_json(merged);
    let mut provenance = Provenance::default();

    for name in tracked() {
        let Some(kept) = json.get(name).filter(|v| !is_empty(v)) else {
            continue;
        };
        let kept = normalized(kept);

        let Some(at) = views
            .iter()
            .position(|view| view.values.get(name) == Some(&kept))
        else {
            continue;
        };

        let named = |pick: &dyn Fn(usize, &View) -> bool| -> Vec<String> {
            views
                .iter()
                .enumerate()
                .filter(|(index, view)| pick(*index, view))
                .map(|(_, view)| view.provider.clone())
                .collect()
        };
        let agreed = named(&|index, view| index > at && view.values.get(name) == Some(&kept));
        let differed = named(&|index, view| {
            index != at && view.values.get(name).is_some_and(|value| *value != kept)
        });
        let passed = named(&|index, view| {
            index < at && !view.values.is_empty() && !view.values.contains_key(name)
        });

        provenance.fields.insert(
            name.to_string(),
            Source {
                from: views[at].provider.clone(),
                agreed,
                differed,
                passed,
            },
        );
    }

    // Counted as they are stored: the seasons' beside the work's, and once
    // per kind and address, the one the table keeps.
    let mut seen = std::collections::HashSet::new();
    let every = merged.images.iter().chain(
        merged
            .seasons
            .iter()
            .flat_map(|season| season.images.iter()),
    );
    for image in every {
        // A picture added by hand is nobody's answer.
        if image.is_manual || !seen.insert((image.cover_type, image.url.as_str())) {
            continue;
        }
        let source = image.source.clone().unwrap_or_else(|| "unknown".into());
        *provenance.images.entry(source).or_default() += 1;
    }

    // As the merge chooses its spine — and TVmaze, whose episodes it sets
    // aside, never is one.
    if !merged.episodes.is_empty() {
        let numbers =
            |view: &&View| view.has_episodes && !DATE_CHECKED.contains(&view.provider.as_str());
        provenance.episodes = views
            .iter()
            .filter(numbers)
            .find(|view| TVDB_NUMBERED.contains(&view.provider.as_str()))
            .or_else(|| views.iter().find(numbers))
            .map(|view| view.provider.clone());
    }

    // The first in priority order that had each: the merge keeps its entry.
    for translation in merged.translations.iter().filter(|t| !t.is_manual) {
        if let Some(view) = views
            .iter()
            .find(|v| v.languages.contains(&translation.language))
        {
            provenance
                .translations
                .insert(translation.language.clone(), view.provider.clone());
        }
    }
    for rating in &merged.ratings {
        if let Some(view) = views.iter().find(|v| v.agencies.contains(&rating.source)) {
            provenance
                .ratings
                .insert(rating.source.clone(), view.provider.clone());
        }
    }
    // One copy of each, the first provider's, by the merge's own rule: the
    // same title, and the same language or none on either side.
    for title in merged.alternative_titles.iter().filter(|t| !t.is_manual) {
        let same = |(other, language): &(String, Option<String>)| {
            other.eq_ignore_ascii_case(&title.title)
                && (*language == title.language || language.is_none() || title.language.is_none())
        };
        if let Some(view) = views.iter().find(|v| v.titles.iter().any(same)) {
            provenance
                .alternative_titles
                .insert(title_key(title), view.provider.clone());
        }
    }

    provenance
}

/// What a merge came back with, as [`carry_over`] needs to know it: before
/// the write puts back what it lacked.
pub struct Returned {
    pictures: bool,
    languages: Vec<String>,
    agencies: Vec<String>,
    titles: Vec<String>,
}

impl Returned {
    pub fn of(merged: &MediaItem) -> Self {
        Self {
            pictures: merged.images.iter().any(|i| !i.is_manual)
                || merged
                    .seasons
                    .iter()
                    .any(|s| s.images.iter().any(|i| !i.is_manual)),
            languages: merged
                .translations
                .iter()
                .map(|t| t.language.clone())
                .collect(),
            agencies: merged.ratings.iter().map(|r| r.source.clone()).collect(),
            titles: merged.alternative_titles.iter().map(title_key).collect(),
        }
    }
}

/// Who gave what the write kept without the merge having it: the lists a
/// refresh keeps when every provider came back without one, and, in a sync,
/// what the stored work carried through. Still whoever gave it before.
pub fn carry_over(now: &mut Provenance, before: &Provenance, returned: &Returned) {
    if now.episodes.is_none() {
        now.episodes.clone_from(&before.episodes);
    }
    if !returned.pictures && now.images.is_empty() {
        now.images.clone_from(&before.images);
    }
    for list in ["credits", "relations"] {
        if !now.fields.contains_key(list)
            && let Some(source) = before.fields.get(list)
        {
            now.fields.insert(list.to_string(), source.clone());
        }
    }

    // Kept whole when the merge had none; entry by entry otherwise, for those
    // still held that nobody gave anew.
    fn entries(
        now: &mut BTreeMap<String, String>,
        before: &BTreeMap<String, String>,
        returned: &[String],
    ) {
        if returned.is_empty() {
            now.clone_from(before);
            return;
        }
        for key in returned {
            if let (false, Some(provider)) = (now.contains_key(key), before.get(key)) {
                now.insert(key.clone(), provider.clone());
            }
        }
    }
    entries(
        &mut now.translations,
        &before.translations,
        &returned.languages,
    );
    entries(&mut now.ratings, &before.ratings, &returned.agencies);
    entries(
        &mut now.alternative_titles,
        &before.alternative_titles,
        &returned.titles,
    );
}

/// Who numbers a work's episode list, as a sync must take it: whoever is on
/// record, when that is TheTVDB or Skyhook; TheTVDB when nobody is and the
/// work has its id; nobody otherwise — TMDB's own list, or a Fan-Kai's, is
/// theirs to give anew.
pub fn numbering_of(stored: &MediaItem, provenance: Option<&Provenance>) -> Option<&'static str> {
    if stored.kind != MediaKind::Series {
        return None;
    }
    match provenance.and_then(|p| p.episodes.as_deref()) {
        Some(recorded) => TVDB_NUMBERED.iter().copied().find(|n| *n == recorded),
        None => stored
            .external_ids
            .tvdb
            .map(|_| crate::providers::names::TVDB),
    }
}

/// The stored episode list stands, in a sync, unless whoever numbers it gave
/// one anew.
///
/// TheTVDB's episodes can fail while the rest of its answer arrives, and a
/// work recorded before its numbering was names nobody for its list: either
/// way, left alone, the merge hands the numbering to whoever else has a list —
/// TMDB's seasons under a TheTVDB id, which is how Sonarr files episodes under
/// the wrong numbers. The list goes to the provider that numbers it, as it
/// would have been had it not been asked.
pub fn protect_numbering(
    contributions: &mut Vec<Contribution>,
    stored: &MediaItem,
    provenance: &Provenance,
) {
    if stored.episodes.is_empty() {
        return;
    }
    let Some(numbering) = numbering_of(stored, Some(provenance)) else {
        return;
    };
    let numbering = numbering.to_string();

    let renumbered = contributions
        .iter()
        .any(|c| TVDB_NUMBERED.contains(&c.provider.as_str()) && !c.item.episodes.is_empty());
    if renumbered {
        return;
    }

    let episodes: Vec<Episode> = stored
        .episodes
        .iter()
        .filter(|e| !e.is_manual)
        .cloned()
        .collect();
    // Without their pictures: those are their own providers' to give back.
    let seasons = || -> Vec<Season> {
        stored
            .seasons
            .iter()
            .filter(|s| !s.is_manual)
            .map(|season| {
                let mut season = season.clone();
                season.images.clear();
                season
            })
            .collect()
    };

    match contributions.iter_mut().find(|c| c.provider == numbering) {
        Some(numbered) => {
            if numbered.item.seasons.is_empty() {
                numbered.item.seasons = seasons();
            }
            numbered.item.episodes = episodes;
        }
        None => {
            let mut item = MediaItem::empty(MediaKind::Series);
            item.seasons = seasons();
            item.episodes = episodes;
            contributions.push(Contribution {
                provider: numbering,
                item,
            });
        }
    }
}

/// The same provenance for a work one provider made alone: the fallbacks
/// that store Skyhook's or Radarr's answer as it came.
pub fn single(provider: &str, item: &MediaItem) -> Provenance {
    attribute(item, &[view(provider, item)])
}

/// What `provider` gave this work last time, as far as the stored work still
/// says: the values it gave or agreed with, its pictures, and the episodes
/// when it was the one numbering them. `None` when nothing of it is left.
///
/// Folded in beside the sources asked again, it answers as it did then, so
/// the merge weighs a fresh value against it by the usual priority — a sync
/// from TheTVDB alone does not hand TheTVDB a title TMDB outranks it on.
pub fn reconstruct(
    stored: &MediaItem,
    provenance: &Provenance,
    provider: &str,
) -> Option<MediaItem> {
    let whole = tracked_json(stored);
    let mut part = serde_json::to_value(MediaItem::empty(stored.kind)).ok()?;
    let mut gave_a_value = false;

    for (name, source) in &provenance.fields {
        let gave = source.from == provider || source.agreed.iter().any(|p| p == provider);
        if let (true, Some(value)) = (gave, whole.get(name)) {
            part[name.as_str()] = value.clone();
            gave_a_value = true;
        }
    }

    let own = |images: &[Image]| -> Vec<Image> {
        images
            .iter()
            .filter(|image| !image.is_manual && image.source.as_deref() == Some(provider))
            .cloned()
            .collect()
    };

    let mut item: MediaItem = serde_json::from_value(part).ok()?;
    item.images = own(&stored.images);
    item.translations = stored
        .translations
        .iter()
        .filter(|t| {
            !t.is_manual
                && provenance
                    .translations
                    .get(&t.language)
                    .is_some_and(|p| p == provider)
        })
        .cloned()
        .collect();
    item.ratings = stored
        .ratings
        .iter()
        .filter(|r| {
            provenance
                .ratings
                .get(&r.source)
                .is_some_and(|p| p == provider)
        })
        .cloned()
        .collect();
    item.alternative_titles = stored
        .alternative_titles
        .iter()
        .filter(|t| {
            !t.is_manual
                && provenance
                    .alternative_titles
                    .get(&title_key(t))
                    .is_some_and(|p| p == provider)
        })
        .cloned()
        .collect();
    // The list whole for whoever numbered it; its seasons' pictures its own
    // only — the others' stay with the stored work, beneath.
    if provenance.episodes.as_deref() == Some(provider) {
        item.seasons = stored
            .seasons
            .iter()
            .filter(|s| !s.is_manual)
            .map(|season| {
                let mut season = season.clone();
                season.images = own(&season.images);
                season
            })
            .collect();
        item.episodes = stored
            .episodes
            .iter()
            .filter(|e| !e.is_manual)
            .cloned()
            .collect();
    }

    let gave_a_list = !item.images.is_empty()
        || !item.episodes.is_empty()
        || !item.translations.is_empty()
        || !item.ratings.is_empty()
        || !item.alternative_titles.is_empty();
    (gave_a_value || gave_a_list).then_some(item)
}

/// The stored work, less the pictures of the sources that answered again:
/// folded in last, under the name [`STORED`], it fills whatever nobody gave
/// anew — a value the source asked again no longer gives is kept rather than
/// lost, until a full refresh settles it — without putting back a picture the
/// source itself has since dropped.
pub fn leftover(stored: &MediaItem, answered: &[&str]) -> MediaItem {
    let kept = |image: &Image| {
        image.is_manual
            || !image
                .source
                .as_deref()
                .is_some_and(|source| answered.contains(&source))
    };

    let mut item = stored.clone();
    item.images.retain(kept);
    for season in &mut item.seasons {
        season.images.retain(kept);
    }
    item
}

fn is_empty(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::String(s) => s.trim().is_empty(),
        serde_json::Value::Array(items) => items.is_empty(),
        _ => false,
    }
}

/// Compared without regard to case or the spaces around it: "Ended" and
/// "ended" are one answer.
fn normalized(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => serde_json::Value::String(s.trim().to_lowercase()),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(normalized).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{CoverType, MediaKind},
        merge::{Contribution, combine},
    };

    fn item(title: &str, status: Option<&str>, runtime: Option<i32>) -> MediaItem {
        let mut item = MediaItem::empty(MediaKind::Series);
        item.title = title.into();
        item.status = status.map(str::to_string);
        item.runtime = runtime;
        item
    }

    fn poster(source: &str, url: &str) -> Image {
        Image {
            id: crate::db::new_id(),
            season_number: None,
            cover_type: CoverType::Poster,
            url: url.into(),
            language: None,
            sort_order: 0,
            source: Some(source.into()),
            is_manual: false,
        }
    }

    fn priority() -> Vec<String> {
        ["tmdb", "tvdb", "skyhook", "fanart"]
            .map(String::from)
            .to_vec()
    }

    /// Merged the way the gatherer merges, with the provenance it records.
    fn merge(answers: Vec<(&str, MediaItem)>) -> (MediaItem, Provenance) {
        let views: Vec<View> = answers.iter().map(|(p, item)| view(p, item)).collect();
        let contributions = answers
            .into_iter()
            .map(|(provider, item)| Contribution {
                provider: provider.into(),
                item,
            })
            .collect();
        let merged = combine(contributions, &priority()).unwrap();
        let provenance = attribute(&merged, &views);
        (merged, provenance)
    }

    #[test]
    fn the_first_that_said_it_gave_it_and_the_rest_agree() {
        let tmdb = item("One Piece", Some("Continuing"), None);
        let tvdb = item("One Piece", Some("continuing"), Some(24));
        let skyhook = item("ONE PIECE (1999)", Some("Continuing"), Some(24));
        let merged = item("One Piece", Some("Continuing"), Some(24));

        let views = [
            view("tmdb", &tmdb),
            view("tvdb", &tvdb),
            view("skyhook", &skyhook),
        ];
        let got = attribute(&merged, &views);

        let title = &got.fields["title"];
        assert_eq!(title.from, "tmdb");
        assert_eq!(title.agreed, vec!["tvdb".to_string()]);
        assert_eq!(title.differed, vec!["skyhook".to_string()]);

        // TMDB had no runtime: TVDB filled it, and says why.
        let runtime = &got.fields["runtime"];
        assert_eq!(runtime.from, "tvdb");
        assert_eq!(runtime.passed, vec!["tmdb".to_string()]);
        // Case aside, the statuses agree.
        assert_eq!(
            got.fields["status"].agreed,
            vec!["tvdb".to_string(), "skyhook".to_string()]
        );
    }

    #[test]
    fn a_value_nobody_gave_is_not_attributed() {
        let tmdb = item("A", None, None);
        let merged = item("A", Some("Ended"), None);
        let got = attribute(&merged, &[view("tmdb", &tmdb)]);
        assert!(!got.fields.contains_key("status"));
        assert_eq!(got.fields["title"].from, "tmdb");
    }

    #[test]
    fn artwork_alone_is_never_what_a_field_passed_over() {
        let mut fanart = MediaItem::empty(MediaKind::Series);
        fanart.images.push(poster("fanart", "https://f/1.jpg"));
        let tvdb = item("A", None, Some(30));
        let merged = item("A", None, Some(30));

        let got = attribute(&merged, &[view("fanart", &fanart), view("tvdb", &tvdb)]);
        assert!(got.fields["runtime"].passed.is_empty());
    }

    /// A sync from TheTVDB alone: its new runtime is taken, and the title TMDB
    /// outranks it on is not — TMDB answers as it did last time.
    #[test]
    fn a_source_synced_alone_is_weighed_against_the_others_as_they_last_answered() {
        let mut tmdb = item("One Piece", Some("Continuing"), None);
        tmdb.images.push(poster("tmdb", "https://t/1.jpg"));
        let mut tvdb = item("ONE PIECE", Some("Continuing"), Some(24));
        tvdb.images.push(poster("tvdb", "https://v/1.jpg"));
        let (stored, provenance) = merge(vec![("tmdb", tmdb), ("tvdb", tvdb)]);
        assert_eq!(stored.runtime, Some(24));

        let mut fresh = item("ONE PIECE", Some("Ended"), Some(25));
        fresh.images.push(poster("tvdb", "https://v/2.jpg"));

        let others: Vec<(&str, MediaItem)> = provenance
            .providers()
            .into_iter()
            .filter(|p| *p != "tvdb")
            .filter_map(|p| Some((p, reconstruct(&stored, &provenance, p)?)))
            .collect();
        assert_eq!(others.len(), 1, "TMDB alone is handed back");

        let mut contributions = vec![Contribution {
            provider: "tvdb".into(),
            item: fresh,
        }];
        contributions.extend(others.into_iter().map(|(provider, item)| Contribution {
            provider: provider.into(),
            item,
        }));
        contributions.push(Contribution {
            provider: STORED.into(),
            item: leftover(&stored, &["tvdb"]),
        });
        let synced = combine(contributions, &priority()).unwrap();

        assert_eq!(
            synced.title, "One Piece",
            "TMDB's title still outranks TheTVDB's"
        );
        assert_eq!(
            synced.status.as_deref(),
            Some("Continuing"),
            "and so does its status"
        );
        assert_eq!(synced.runtime, Some(25), "TheTVDB's new runtime is taken");

        let urls: Vec<&str> = synced.images.iter().map(|i| i.url.as_str()).collect();
        assert_eq!(
            urls,
            ["https://t/1.jpg", "https://v/2.jpg"],
            "TheTVDB's old poster is gone"
        );
    }

    /// Season pictures live on the seasons: counted there, handed back to
    /// their own provider only, and dropped when that provider answers again.
    #[test]
    fn season_pictures_follow_their_own_source() {
        fn season(number: i32, images: Vec<Image>) -> crate::domain::Season {
            crate::domain::Season {
                id: crate::db::new_id(),
                season_number: number,
                title: None,
                overview: None,
                air_date: None,
                tmdb_id: None,
                tvdb_id: None,
                is_manual: false,
                images,
            }
        }
        fn with_episode(mut item: MediaItem) -> MediaItem {
            item.episodes.push(crate::domain::Episode {
                id: crate::db::new_id(),
                season_number: 1,
                episode_number: 1,
                absolute_episode_number: None,
                aired_after_season_number: None,
                aired_before_season_number: None,
                aired_before_episode_number: None,
                title: "Pilot".into(),
                overview: None,
                air_date: None,
                air_date_utc: None,
                runtime: None,
                finale_type: None,
                image: None,
                tvdb_id: None,
                tmdb_id: None,
                rating: None,
                is_manual: false,
            });
            item
        }

        let mut tvdb = with_episode(item("A", None, None));
        tvdb.seasons
            .push(season(1, vec![poster("tvdb", "https://v/s1.jpg")]));
        let mut tmdb = item("A", None, None);
        tmdb.seasons
            .push(season(1, vec![poster("tmdb", "https://t/s1-old.jpg")]));
        let (stored, provenance) = merge(vec![("tmdb", tmdb), ("tvdb", tvdb)]);
        assert_eq!(provenance.images.get("tmdb"), Some(&1));
        assert_eq!(provenance.images.get("tvdb"), Some(&1));

        let spine = reconstruct(&stored, &provenance, "tvdb").unwrap();
        let urls: Vec<&str> = spine.seasons[0]
            .images
            .iter()
            .map(|i| i.url.as_str())
            .collect();
        assert_eq!(
            urls,
            ["https://v/s1.jpg"],
            "TheTVDB gets its own season pictures back, no one else's"
        );

        let mut fresh = item("A", None, None);
        fresh
            .seasons
            .push(season(1, vec![poster("tmdb", "https://t/s1-new.jpg")]));
        let synced = combine(
            vec![
                Contribution {
                    provider: "tmdb".into(),
                    item: fresh,
                },
                Contribution {
                    provider: "tvdb".into(),
                    item: spine,
                },
                Contribution {
                    provider: STORED.into(),
                    item: leftover(&stored, &["tmdb"]),
                },
            ],
            &priority(),
        )
        .unwrap();
        let mut urls: Vec<&str> = synced.seasons[0]
            .images
            .iter()
            .map(|i| i.url.as_str())
            .collect();
        urls.sort_unstable();
        assert_eq!(
            urls,
            ["https://t/s1-new.jpg", "https://v/s1.jpg"],
            "TMDB's old season poster is gone"
        );
    }

    fn episode(season: i32, number: i32, overview: &str) -> Episode {
        Episode {
            id: crate::db::new_id(),
            season_number: season,
            episode_number: number,
            absolute_episode_number: None,
            aired_after_season_number: None,
            aired_before_season_number: None,
            aired_before_episode_number: None,
            title: format!("{season}x{number}"),
            overview: (!overview.is_empty()).then(|| overview.to_string()),
            air_date: None,
            air_date_utc: None,
            runtime: None,
            finale_type: None,
            image: None,
            tvdb_id: None,
            tmdb_id: None,
            rating: None,
            is_manual: false,
        }
    }

    fn translation(language: &str, title: &str) -> crate::domain::Translation {
        crate::domain::Translation {
            language: language.into(),
            title: Some(title.into()),
            overview: None,
            is_manual: false,
        }
    }

    fn rating(source: &str, value: f64) -> crate::domain::Rating {
        crate::domain::Rating {
            source: source.into(),
            value: Some(value),
            votes: Some(100),
            rating_type: None,
        }
    }

    /// The merge a sync makes, as `gather::resync` makes it: the others handed
    /// back, the list kept, the stored work beneath.
    fn synced(
        stored: &MediaItem,
        provenance: &Provenance,
        fresh: Vec<(&str, MediaItem)>,
    ) -> (MediaItem, Provenance) {
        let answered: Vec<&str> = fresh.iter().map(|(p, _)| *p).collect();
        let mut contributions: Vec<Contribution> = fresh
            .into_iter()
            .map(|(provider, item)| Contribution {
                provider: provider.into(),
                item,
            })
            .collect();
        for provider in provenance.providers() {
            if !answered.contains(&provider)
                && let Some(item) = reconstruct(stored, provenance, provider)
            {
                contributions.push(Contribution {
                    provider: provider.into(),
                    item,
                });
            }
        }
        protect_numbering(&mut contributions, stored, provenance);
        // In priority order, as `views_of` takes them.
        let rank = |p: &str| priority().iter().position(|q| q == p).unwrap_or(usize::MAX);
        contributions.sort_by_key(|c| rank(&c.provider));
        let views: Vec<View> = contributions
            .iter()
            .map(|c| view(&c.provider, &c.item))
            .collect();
        contributions.push(Contribution {
            provider: STORED.into(),
            item: leftover(stored, &answered),
        });
        let merged = combine(contributions, &priority()).unwrap();
        let mut now = attribute(&merged, &views);
        carry_over(&mut now, provenance, &Returned::of(&merged));
        (merged, now)
    }

    fn numbers(item: &MediaItem) -> Vec<(i32, i32)> {
        item.episodes
            .iter()
            .map(|e| (e.season_number, e.episode_number))
            .collect()
    }

    /// TheTVDB's episodes failing while the rest of its answer arrives must not
    /// hand the numbering to TMDB.
    #[test]
    fn a_numbering_source_without_its_episodes_keeps_the_stored_list() {
        let mut tvdb = item("A", None, None);
        tvdb.episodes = vec![episode(1, 1, ""), episode(1, 2, ""), episode(1, 3, "")];
        let mut tmdb = item("A", None, None);
        tmdb.episodes = vec![
            episode(1, 1, "one"),
            episode(1, 2, "two"),
            episode(2, 1, "three"),
        ];
        let (stored, provenance) = merge(vec![("tmdb", tmdb.clone()), ("tvdb", tvdb)]);
        assert_eq!(provenance.episodes.as_deref(), Some("tvdb"));

        let failed = item("A", None, None);
        let (after, now) = synced(&stored, &provenance, vec![("tmdb", tmdb), ("tvdb", failed)]);
        assert_eq!(numbers(&after), [(1, 1), (1, 2), (1, 3)]);
        assert_eq!(now.episodes.as_deref(), Some("tvdb"));
    }

    /// A list nobody was named for, on a work with a TheTVDB id, is TheTVDB's
    /// to renumber, not TMDB's.
    #[test]
    fn an_unattributed_list_stands_against_a_fresh_one_from_elsewhere() {
        let mut stored = item("A", None, None);
        stored.external_ids.tvdb = Some(81189);
        stored.episodes = vec![episode(1, 1, ""), episode(1, 2, ""), episode(1, 3, "")];
        let provenance = Provenance::default();

        let mut tmdb = item("A", None, None);
        tmdb.episodes = vec![episode(1, 1, "one"), episode(2, 1, "two")];
        let (after, _) = synced(&stored, &provenance, vec![("tmdb", tmdb)]);
        assert_eq!(numbers(&after), [(1, 1), (1, 2), (1, 3)]);
        assert_eq!(
            after.episodes[0].overview.as_deref(),
            Some("one"),
            "TMDB still fills it in"
        );
    }

    /// A lower source synced alone does not take the translations and ratings
    /// a higher one gave.
    #[test]
    fn translations_and_ratings_go_back_to_whoever_gave_them() {
        let mut tmdb = item("A", None, None);
        tmdb.translations = vec![translation("fra", "Le titre de TMDB")];
        tmdb.ratings = vec![rating("tmdb", 8.1)];
        let mut tvdb = item("A", None, None);
        tvdb.translations = vec![
            translation("fra", "Le titre de TheTVDB"),
            translation("deu", "Der Titel"),
        ];
        let (stored, provenance) = merge(vec![("tmdb", tmdb), ("tvdb", tvdb.clone())]);
        assert_eq!(provenance.translations["fra"], "tmdb");
        assert_eq!(provenance.translations["deu"], "tvdb");
        assert_eq!(provenance.ratings["tmdb"], "tmdb");

        let mut radarr_like = tvdb;
        radarr_like.ratings = vec![rating("tmdb", 5.0)];
        let (after, now) = synced(&stored, &provenance, vec![("tvdb", radarr_like)]);
        let french = after
            .translations
            .iter()
            .find(|t| t.language == "fra")
            .unwrap();
        assert_eq!(french.title.as_deref(), Some("Le titre de TMDB"));
        assert_eq!(
            after
                .ratings
                .iter()
                .find(|r| r.source == "tmdb")
                .unwrap()
                .value,
            Some(8.1)
        );
        assert_eq!(now.translations["fra"], "tmdb");
    }

    /// What a refresh keeps when every provider came back without it is still
    /// whoever gave it.
    #[test]
    fn a_kept_list_keeps_its_source() {
        let before = Provenance {
            episodes: Some("tvdb".into()),
            ..Provenance::default()
        };
        let mut now = Provenance::default();
        carry_over(&mut now, &before, &Returned::of(&item("A", None, None)));
        assert_eq!(now.episodes.as_deref(), Some("tvdb"));
    }

    /// TVmaze's episodes are set aside by the merge: it never numbers a list,
    /// whatever the priority.
    #[test]
    fn tvmaze_is_never_what_numbered_the_list() {
        let mut tvmaze = item("A", None, None);
        tvmaze.episodes = vec![episode(1, 1, "")];
        let mut tmdb = item("A", None, None);
        tmdb.episodes = vec![episode(1, 1, "")];
        let merged = tmdb.clone();
        let got = attribute(&merged, &[view("tvmaze", &tvmaze), view("tmdb", &tmdb)]);
        assert_eq!(got.episodes.as_deref(), Some("tmdb"));
    }

    /// The light form attribution compares is the work's own, member for
    /// member: a field added to the registry and not to `Tracked` fails here.
    #[test]
    fn the_tracked_members_are_serialised_as_the_work_serialises_them() {
        let mut item = MediaItem::empty(MediaKind::Movie);
        item.title = "T".into();
        for (name, value) in [
            ("sortTitle", "s"),
            ("originalTitle", "o"),
            ("overview", "v"),
            ("status", "ended"),
            ("originalLanguage", "eng"),
            ("originalCountry", "usa"),
            ("firstAired", "2001-01-01"),
            ("lastAired", "2002-01-01"),
            ("inCinemas", "2001-02-02"),
            ("physicalRelease", "2001-03-03"),
            ("digitalRelease", "2001-04-04"),
            ("airTime", "21:00"),
            ("network", "N"),
            ("studio", "S"),
            ("contentRating", "TV-MA"),
            ("contentRatingCountry", "US"),
            ("homepage", "https://h"),
            ("trailerYoutubeId", "yt"),
            ("themeMusic", "https://m"),
        ] {
            let mut json = serde_json::to_value(&item).unwrap();
            json[name] = serde_json::Value::String(value.into());
            item = serde_json::from_value(json).unwrap();
        }
        item.runtime = Some(50);
        item.year = Some(2001);
        item.popularity = Some(1.5);
        item.collection_tmdb_id = Some(7);
        item.genres = vec!["Drama".into()];
        item.keywords = vec!["k".into()];
        item.credits = serde_json::from_value(serde_json::json!([
            { "id": "c", "personName": "P", "creditType": "actor", "sortOrder": 0, "isManual": false }
        ]))
        .unwrap();
        assert_eq!(item.credits.len(), 1);

        let whole = serde_json::to_value(&item).unwrap();
        let light = tracked_json(&item);
        for name in tracked() {
            assert_eq!(light.get(name), whole.get(name), "{name}");
            if name != "credits" && name != "relations" {
                assert!(
                    whole.get(name).is_some(),
                    "{name} is filled for the comparison"
                );
            }
        }
    }

    /// A lower source synced alone does not take the country a higher one
    /// filed an alternative title under.
    #[test]
    fn alternative_titles_go_back_to_whoever_gave_them() {
        fn titled(title: &str, language: Option<&str>) -> AlternativeTitle {
            AlternativeTitle {
                id: crate::db::new_id(),
                title: title.into(),
                title_type: None,
                language: language.map(str::to_string),
                is_manual: false,
            }
        }
        let mut tmdb = item("A", None, None);
        tmdb.alternative_titles = vec![titled("Le Show", Some("fra"))];
        let mut tvdb = item("A", None, None);
        tvdb.alternative_titles = vec![titled("Le Show", None)];
        let (stored, provenance) = merge(vec![("tmdb", tmdb), ("tvdb", tvdb.clone())]);
        assert_eq!(provenance.alternative_titles["le show|fra"], "tmdb");

        let (after, _) = synced(&stored, &provenance, vec![("tvdb", tvdb)]);
        let kept: Vec<_> = after
            .alternative_titles
            .iter()
            .map(|t| (t.title.as_str(), t.language.as_deref()))
            .collect();
        assert_eq!(kept, [("Le Show", Some("fra"))]);
    }

    #[test]
    fn the_numbering_is_whoever_is_on_record_or_thetvdb_by_its_id() {
        let mut series = item("A", None, None);
        assert_eq!(numbering_of(&series, None), None);
        series.external_ids.tvdb = Some(1);
        assert_eq!(numbering_of(&series, None), Some("tvdb"));

        let recorded = |who: &str| Provenance {
            episodes: Some(who.into()),
            ..Provenance::default()
        };
        assert_eq!(
            numbering_of(&series, Some(&recorded("skyhook"))),
            Some("skyhook")
        );
        assert_eq!(numbering_of(&series, Some(&recorded("tmdb"))), None);

        let film = MediaItem::empty(MediaKind::Movie);
        assert_eq!(numbering_of(&film, Some(&recorded("tvdb"))), None);
    }

    #[test]
    fn nothing_left_of_a_provider_is_nothing_to_hand_back() {
        let tmdb = item("A", None, None);
        let (stored, provenance) = merge(vec![("tmdb", tmdb)]);
        assert!(reconstruct(&stored, &provenance, "tvdb").is_none());
        assert!(reconstruct(&stored, &provenance, "tmdb").is_some());
    }
}
