//! The Fankai wiki: which anime each Fan-Kai was cut from.
//!
//! Fankai's metadata service describes a production and nothing beside it —
//! the ids it lists for one are blank. The community's wiki, at
//! fan-kai.fandom.com, keeps a page per Fan-Kai whose infobox links the
//! original's AniList and MyAnimeList entries, and names the Fan-Kai that
//! follows it. That is what lets a Fan-Kai lead to the anime it was cut from,
//! and the anime to its Fan-Kai, by identifier rather than by a guessed title.
//!
//! One page per production fetch, through MediaWiki's API: a search on the
//! production's name that brings back the pages' text in the same answer. The
//! wiki keeps a page per cut — *Naruto Shippuden Yabai (Triggerforce)* and
//! *Naruto Shippuden Kai (Mixouille)* are two — and sometimes a page per
//! language, so a page is taken on its exact name first, then on the name of
//! whoever cut it, and only then alone of its name. Its content is CC BY-SA;
//! the interface credits it when this source is on.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    config,
    domain::Relation,
    providers::{PATIENCE, Pacer, fankai::fold, names},
};

/// Pages brought back by one search. The page wanted is nearly always first;
/// the rest are its namesakes, which is what the choice needs to see.
const RESULTS: &str = "8";

pub struct FankaiWikiClient {
    http: reqwest::Client,
    api: String,
    pacer: Pacer,
}

/// What a production's page says, as far as this server reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub title: String,
    /// The anime it was cut from — one entry per season a site splits it into.
    pub originals: Vec<Original>,
    /// The wiki's pages for the Fan-Kai that follow it.
    pub sequels: Vec<String>,
}

/// One entry of the anime a Fan-Kai was cut from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Original {
    pub anilist: Option<i64>,
    pub mal: Option<i64>,
    /// The entry's name as the link spells it, for when nothing better is known.
    pub name: Option<String>,
}

impl Original {
    /// The entry as a relation of the Fan-Kai, from what the wiki says alone.
    pub fn relation(&self) -> Option<Relation> {
        let (source, external_id, site) = match (self.anilist, self.mal) {
            (Some(id), _) => ("anilist", id, "AniList"),
            (None, Some(id)) => ("mal", id, "MyAnimeList"),
            (None, None) => return None,
        };

        Some(Relation {
            id: String::new(),
            relation_type: "ORIGINAL".to_string(),
            source: source.to_string(),
            external_id,
            mal_id: self.mal,
            title: self
                .name
                .clone()
                .unwrap_or_else(|| format!("{site} {external_id}")),
            medium: "anime".to_string(),
            format: None,
            year: None,
            image: None,
            is_adult: false,
            work_id: None,
            sort_order: 0,
        })
    }
}

impl FankaiWikiClient {
    pub fn new(http: reqwest::Client, cfg: &config::FankaiWiki) -> Self {
        Self {
            http,
            api: cfg.upstream.clone(),
            // A wiki, not an API service: one call a second.
            pacer: Pacer::new(Duration::from_secs(1)),
        }
    }

    /// The page for a production, found by its name and, where the wiki keeps
    /// one page per cut, by whoever cut it. Returns the page's text alongside
    /// what was read from it.
    /// The page's own address, for a reader: beside the API this client asks,
    /// where MediaWiki lays out its pages.
    pub fn page_url(&self, title: &str) -> Option<String> {
        let api = url::Url::parse(&self.api).ok()?;
        // As the wiki spells titles in its addresses; `?` and `#` would end
        // the path.
        let path = title
            .trim()
            .replace(' ', "_")
            .replace('%', "%25")
            .replace('?', "%3F")
            .replace('#', "%23");
        api.join(&format!("wiki/{path}")).ok().map(String::from)
    }

    pub async fn page(&self, title: &str, kaieurs: &[&str]) -> Result<Option<(Value, Page)>> {
        if title.trim().is_empty() {
            return Ok(None);
        }

        if !self.pacer.turn(1, PATIENCE).await {
            anyhow::bail!("the Fankai wiki's queue is full; this fetch goes without it");
        }

        let started = std::time::Instant::now();
        let response = self
            .http
            .get(&self.api)
            .query(&[
                ("action", "query"),
                ("format", "json"),
                ("formatversion", "2"),
                ("generator", "search"),
                ("gsrsearch", title.trim()),
                ("gsrnamespace", "0"),
                ("gsrlimit", RESULTS),
                ("prop", "revisions"),
                ("rvprop", "content"),
                ("rvslots", "main"),
            ])
            .timeout(Duration::from_secs(20))
            .send()
            .await;
        crate::metrics::upstream(
            names::FANKAI_WIKI,
            started,
            response.as_ref().ok().map(|r| r.status()),
        );
        let response = response.context("the Fankai wiki could not be reached")?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            anyhow::bail!("the Fankai wiki's rate limit was reached");
        }
        if !status.is_success() {
            anyhow::bail!("the Fankai wiki returned {status}");
        }

        let raw = crate::providers::read_json(response)
            .await
            .context("the Fankai wiki returned a body this server could not read")?;
        let pages = results(&raw);

        let Some(chosen) = choose(&pages, title, kaieurs) else {
            return Ok(None);
        };
        let Some(fields) = infobox(&chosen.text) else {
            return Ok(None);
        };

        let field = |name: &str| {
            fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        let page = Page {
            title: chosen.title.clone(),
            originals: field("al_-_mal").map(originals).unwrap_or_default(),
            sequels: field("suite").map(links).unwrap_or_default(),
        };

        // Only the page read is kept, not the search around it.
        let kept = json!({ "page": chosen.title, "wikitext": chosen.text });
        Ok(Some((kept, page)))
    }
}

// ─── the wire ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Answer {
    query: Option<Query>,
}

#[derive(Debug, Deserialize)]
struct Query {
    #[serde(default)]
    pages: Vec<PageRecord>,
}

#[derive(Debug, Deserialize)]
struct PageRecord {
    title: String,
    /// The search's own ranking; the list itself is in id order.
    index: Option<i64>,
    #[serde(default)]
    revisions: Vec<Revision>,
}

#[derive(Debug, Deserialize)]
struct Revision {
    slots: Option<Slots>,
}

#[derive(Debug, Deserialize)]
struct Slots {
    main: Option<Slot>,
}

#[derive(Debug, Deserialize)]
struct Slot {
    content: Option<String>,
}

/// A page the search brought back, with its text.
#[derive(Debug)]
struct Found {
    title: String,
    text: String,
}

/// The pages of a search, best ranked first. A search that found nothing
/// answers without a `query` at all.
fn results(raw: &Value) -> Vec<Found> {
    let Ok(answer) = Answer::deserialize(raw) else {
        return Vec::new();
    };

    let mut pages: Vec<(i64, Found)> = answer
        .query
        .map(|q| q.pages)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|page| {
            let text = page.revisions.into_iter().next()?.slots?.main?.content?;
            Some((
                page.index.unwrap_or(i64::MAX),
                Found {
                    title: page.title,
                    text,
                },
            ))
        })
        .collect();

    pages.sort_by_key(|(index, _)| *index);
    pages.into_iter().map(|(_, page)| page).collect()
}

// ─── reading a page ──────────────────────────────────────────────────────────

/// The page for `title`, among the ones a search found.
///
/// Its exact name first — *Horimiya Kai*, not *Horimiya Kai (English)* — then
/// the one qualified by the name of whoever cut it, then one alone of its
/// name. Two of a name that disagree on what they were cut from are not
/// guessed between.
fn choose<'a>(pages: &'a [Found], title: &str, kaieurs: &[&str]) -> Option<&'a Found> {
    let wanted = fold(title);
    let cut_by: Vec<String> = kaieurs.iter().map(|k| fold(k)).collect();
    let described: Vec<&Found> = pages
        .iter()
        .filter(|p| infobox(&p.text).is_some())
        .collect();

    if let Some(exact) = described.iter().find(|p| fold(&p.title) == wanted) {
        return Some(exact);
    }

    let namesakes: Vec<&Found> = described
        .into_iter()
        .filter(|p| fold(base(&p.title)) == wanted)
        .collect();

    if let Some(theirs) = namesakes
        .iter()
        .find(|p| qualifier(&p.title).is_some_and(|q| cut_by.contains(&fold(q))))
    {
        return Some(theirs);
    }

    // What each namesake was cut from, on both sites: two pages that link
    // only MyAnimeList still differ by the entry they link.
    let first = namesakes.first()?;
    let entries = |page: &Found| -> Vec<(Option<i64>, Option<i64>)> {
        infobox(&page.text)
            .and_then(|fields| {
                fields
                    .into_iter()
                    .find(|(key, _)| key == "al_-_mal")
                    .map(|(_, value)| {
                        originals(&value)
                            .into_iter()
                            .map(|o| (o.anilist, o.mal))
                            .collect()
                    })
            })
            .unwrap_or_default()
    };
    let agreed = entries(first);
    namesakes
        .iter()
        .all(|page| entries(page) == agreed)
        .then_some(*first)
}

/// A page's name without the qualifier that tells namesakes apart: the name
/// Fankai gives the production.
pub fn base_name(title: &str) -> &str {
    base(title)
}

/// A page's name without the qualifier that tells namesakes apart.
fn base(title: &str) -> &str {
    match title.trim_end().strip_suffix(')') {
        Some(open) => open
            .rfind(" (")
            .map_or(title.trim_end(), |at| title[..at].trim_end()),
        None => title.trim_end(),
    }
}

/// The qualifier itself: the kaïeur, a language.
fn qualifier(title: &str) -> Option<&str> {
    let open = title.trim_end().strip_suffix(')')?;
    let at = open.rfind(" (")?;
    Some(open[at + 2..].trim())
}

/// The fields of the page's `Fiche Série`, as `(name, value)` with the name in
/// lower case — or nothing, for a page that has none.
///
/// A value may hold links and templates of its own, `{{!}}` among them, so
/// the fields are split only on the bars of the infobox itself.
fn infobox(wikitext: &str) -> Option<Vec<(String, String)>> {
    let start = ["{{Fiche_S", "{{Fiche S", "{{fiche_s", "{{fiche s"]
        .iter()
        .filter_map(|marker| wikitext.find(marker))
        .min()?;

    let body = &wikitext[start + 2..];
    let mut templates = 1usize;
    let mut links = 0usize;
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut at = 0;

    while at < body.len() {
        let rest = &body[at..];
        let step = if rest.starts_with("{{") {
            templates += 1;
            2
        } else if rest.starts_with("}}") {
            templates -= 1;
            if templates == 0 {
                break;
            }
            2
        } else if rest.starts_with("[[") {
            links += 1;
            2
        } else if rest.starts_with("]]") {
            links = links.saturating_sub(1);
            2
        } else if rest.starts_with('|') && templates == 1 && links == 0 {
            parts.push(std::mem::take(&mut current));
            at += 1;
            continue;
        } else {
            rest.chars().next().map_or(1, char::len_utf8)
        };

        current.push_str(&rest[..step]);
        at += step;
    }
    parts.push(current);

    // The first part is the template's own name.
    let fields: Vec<(String, String)> = parts
        .into_iter()
        .skip(1)
        .filter_map(|part| {
            let (name, value) = part.split_once('=')?;
            Some((name.trim().to_lowercase(), value.trim().to_string()))
        })
        .collect();

    Some(fields)
}

/// The entries an `al_-_mal` field links, in order: `[AniList AL] {{!}}
/// [MyAnimeList MAL]`, once per entry.
///
/// As many links to each site pair up in order; otherwise a MyAnimeList link
/// goes with the AniList link before it.
fn originals(value: &str) -> Vec<Original> {
    let anilist = ids_after(value, "anilist.co/anime/");
    let mal = ids_after(value, "myanimelist.net/anime/");

    if anilist.len() == mal.len() {
        return anilist
            .into_iter()
            .zip(mal)
            .map(|((_, a, a_name), (_, m, m_name))| Original {
                anilist: Some(a),
                mal: Some(m),
                name: m_name.or(a_name),
            })
            .collect();
    }

    let mut links: Vec<(usize, bool, i64, Option<String>)> = anilist
        .into_iter()
        .map(|(at, id, name)| (at, true, id, name))
        .chain(mal.into_iter().map(|(at, id, name)| (at, false, id, name)))
        .collect();
    links.sort_by_key(|(at, ..)| *at);

    let mut out: Vec<Original> = Vec::new();
    for (_, is_anilist, id, name) in links {
        match out.last_mut() {
            Some(last) if !is_anilist && last.mal.is_none() && last.anilist.is_some() => {
                last.mal = Some(id);
                last.name = name.or(last.name.take());
            }
            _ => out.push(Original {
                anilist: is_anilist.then_some(id),
                mal: (!is_anilist).then_some(id),
                name,
            }),
        }
    }
    out
}

/// Every `marker<digits>[/slug]` in `text`: where it is, the id, and the slug
/// made readable.
fn ids_after(text: &str, marker: &str) -> Vec<(usize, i64, Option<String>)> {
    let mut found = Vec::new();
    let mut from = 0;

    while let Some(offset) = text[from..].find(marker) {
        let at = from + offset;
        let rest = &text[at + marker.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        from = at + marker.len();

        let Ok(id) = digits.parse::<i64>() else {
            continue;
        };
        let slug = rest[digits.len()..]
            .strip_prefix('/')
            .map(|s| {
                s.split(['/', ' ', ']', '?', '#', '|'])
                    .next()
                    .unwrap_or_default()
            })
            .map(readable)
            .filter(|s| !s.is_empty());

        if id > 0 && !found.iter().any(|(_, known, _)| *known == id) {
            found.push((at, id, slug));
        }
    }

    found
}

/// The pages a field links to, by name: `[[Page]]`, `[[Page|text]]`, or an
/// address on the wiki with its text beside it.
fn links(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;

    while let Some(open) = rest.find('[') {
        rest = &rest[open..];

        let name = if let Some(inner) = rest.strip_prefix("[[") {
            let Some(close) = inner.find("]]") else {
                break;
            };
            let name = inner[..close].split('|').next().unwrap_or_default();
            rest = &inner[close + 2..];
            name.to_string()
        } else {
            let inner = &rest[1..];
            let Some(close) = inner.find(']') else {
                break;
            };
            // An address and the text shown for it; the address names the page.
            let link = &inner[..close];
            let (address, text) = link.split_once(' ').unwrap_or((link, ""));
            rest = &inner[close + 1..];
            address
                .split("/wiki/")
                .nth(1)
                .map(|page| decoded(page, false))
                .unwrap_or_else(|| text.trim().to_string())
        };

        let name = name.trim().to_string();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }

    out
}

/// A slug as words: `Naruto__Shippuuden` is `Naruto Shippuuden`, and
/// AniList's `NARUTO-Shippuuden` is `NARUTO Shippuuden`.
fn readable(slug: &str) -> String {
    decoded(slug, true)
}

/// Percent-escapes decoded and underscores read as spaces — dashes too, for a
/// slug, where they stand for one; never in a page's name, where they are the
/// name's own: *D.Gray-man*.
fn decoded(text: &str, dashes: bool) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;

    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(hex) = text.get(at + 1..at + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            at += 3;
            continue;
        }
        let spaced = bytes[at] == b'_' || (dashes && bytes[at] == b'-');
        out.push(if spaced { b' ' } else { bytes[at] });
        at += 1;
    }

    String::from_utf8_lossy(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NICHIJOU: &str = "<br />{{Fiche_Série|image1=Nichijou henshu affiche.png|kaieur=[https://fan-kai.fandom.com/fr/wiki/Cat%C3%A9gorie:Roro Roro]|type=[https://fan-kai.fandom.com/fr/wiki/Lexique Henshū]|statut=Terminé|source=Manga|épisodes=26|films=6|qualité=1080p|langue(s)=VOSTFR|al_-_mal=[https://anilist.co/anime/10165/Nichijou/ AL] {{!}} [https://myanimelist.net/anime/10165/Nichijou MAL]|hors-séries=Non disponible|temps-kai=05h49\n10h24|equivalence-episodes=15}}\n\n''La série suit la vie quotidienne.''\n{| class=\"fandom-table\"\n!FILMS\n|-\n|1 - Statuettes\n|}";

    const NARUTO: &str = "{{Fiche_Série|image1=Naruto yabai affiche.png|kaieur=[https://fan-kai.fandom.com/fr/wiki/Cat%C3%A9gorie:Triggerforce Triggerforce]|type=[https://fan-kai.fandom.com/fr/wiki/Lexique Yabai]|statut=Terminé|suite=[https://fan-kai.fandom.com/fr/wiki/Naruto_Shippuden_Yabai_(Triggerforce) Naruto Shippuden Yabai (Triggerforce)]|source=Manga|épisodes=220|films=18|qualité=1080p ([https://fan-kai.fandom.com/fr/wiki/Lexique upscale])|langue(s)=Multi-dubs (JP, FR)\nMulti-subs (FR, EN, IT, PO, ES)|al_-_mal=[https://anilist.co/anime/20/NARUTO/ AL] {{!}} [https://myanimelist.net/anime/20/Naruto MAL]|hors-séries=[https://www.animefillerlist.com/shows/naruto Lien]}}\n\nDans le village de Konoha.";

    fn found(title: &str, anilist: i64) -> Found {
        Found {
            title: title.to_string(),
            text: format!(
                "{{{{Fiche_Série|al_-_mal=[https://anilist.co/anime/{anilist}/X AL] {{{{!}}}} [https://myanimelist.net/anime/{anilist}/X MAL]}}}}"
            ),
        }
    }

    #[test]
    fn the_infobox_is_split_on_its_own_bars_only() {
        let fields = infobox(NICHIJOU).unwrap();
        let field = |name: &str| {
            fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };

        assert_eq!(field("statut"), Some("Terminé"));
        assert_eq!(field("épisodes"), Some("26"));
        assert_eq!(field("temps-kai"), Some("05h49\n10h24"));
        // The escaped bar inside a value does not split it.
        assert_eq!(
            field("al_-_mal"),
            Some(
                "[https://anilist.co/anime/10165/Nichijou/ AL] {{!}} [https://myanimelist.net/anime/10165/Nichijou MAL]"
            )
        );
        assert_eq!(infobox("No infobox here. {{Other|a=b}}"), None);
    }

    #[test]
    fn the_original_is_read_off_its_links() {
        let fields = infobox(NARUTO).unwrap();
        let value = &fields.iter().find(|(k, _)| k == "al_-_mal").unwrap().1;

        assert_eq!(
            originals(value),
            vec![Original {
                anilist: Some(20),
                mal: Some(20),
                name: Some("Naruto".into()),
            }]
        );

        // Two seasons, each linked on both sites, pair up in order.
        let two = "[https://anilist.co/anime/16498/Shingeki-no-Kyojin/ AL] {{!}} [https://myanimelist.net/anime/16498/Shingeki_no_Kyojin MAL]<br>[https://anilist.co/anime/20958/ AL] {{!}} [https://myanimelist.net/anime/25777/Shingeki_no_Kyojin_Season_2 MAL]";
        let read = originals(two);
        assert_eq!(read.len(), 2);
        assert_eq!((read[1].anilist, read[1].mal), (Some(20958), Some(25777)));
        assert_eq!(read[1].name.as_deref(), Some("Shingeki no Kyojin Season 2"));

        // A page that only links MyAnimeList still names the entry.
        let lone = originals("[https://myanimelist.net/anime/889/Black_Lagoon MAL]");
        assert_eq!(
            lone,
            vec![Original {
                anilist: None,
                mal: Some(889),
                name: Some("Black Lagoon".into()),
            }]
        );
        assert_eq!(lone[0].relation().unwrap().source, "mal");
        assert!(originals("Non disponible").is_empty());
    }

    #[test]
    fn a_sequel_is_named_by_the_page_it_links() {
        let fields = infobox(NARUTO).unwrap();
        let value = &fields.iter().find(|(k, _)| k == "suite").unwrap().1;
        assert_eq!(links(value), vec!["Naruto Shippuden Yabai (Triggerforce)"]);

        assert_eq!(
            links("[https://fan-kai.fandom.com/fr/wiki/Boruto_Ka%C3%AF Boruto Kaï]"),
            vec!["Boruto Kaï"]
        );
        assert_eq!(
            links("[[Boruto Kaï|la suite]] et [[Two]]"),
            vec!["Boruto Kaï", "Two"]
        );
        assert!(links("Aucune").is_empty());
    }

    #[test]
    fn a_page_is_chosen_by_its_exact_name_then_by_who_cut_it() {
        let pages = vec![
            found("Horimiya Kai (English)", 124080),
            found("Horimiya Kai", 124080),
        ];
        assert_eq!(
            choose(&pages, "Horimiya Kaï", &[]).unwrap().title,
            "Horimiya Kai"
        );

        let pages = vec![
            found("Naruto Shippuden Kai (Mixouille)", 1735),
            found("Naruto Shippuden Yabai (Triggerforce)", 1735),
            found("Naruto Yabai (Triggerforce)", 20),
        ];
        assert_eq!(
            choose(&pages, "Naruto Shippuden Yabai", &["Triggerforce"])
                .unwrap()
                .title,
            "Naruto Shippuden Yabai (Triggerforce)"
        );
        // Alone of its name, it is taken without the kaïeur.
        assert_eq!(
            choose(&pages, "Naruto Shippuden Yabai", &[]).unwrap().title,
            "Naruto Shippuden Yabai (Triggerforce)"
        );
        assert!(choose(&pages, "One Piece Kaï", &[]).is_none());
    }

    #[test]
    fn namesakes_that_disagree_are_not_guessed_between() {
        let pages = vec![found("Kai (Alice)", 1), found("Kai (Bob)", 2)];
        assert!(choose(&pages, "Kai", &[]).is_none());
        assert_eq!(choose(&pages, "Kai", &["Bob"]).unwrap().title, "Kai (Bob)");

        let agreeing = vec![found("Kai (Alice)", 1), found("Kai (Bob)", 1)];
        assert_eq!(choose(&agreeing, "Kai", &[]).unwrap().title, "Kai (Alice)");

        // Linked on MyAnimeList alone, two different originals still disagree.
        let mal_only = |title: &str, mal: i64| Found {
            title: title.to_string(),
            text: format!(
                "{{{{Fiche_Série|al_-_mal=[https://myanimelist.net/anime/{mal}/X MAL]}}}}"
            ),
        };
        let pages = vec![mal_only("Kai (Alice)", 5), mal_only("Kai (Bob)", 6)];
        assert!(choose(&pages, "Kai", &[]).is_none());
    }

    #[test]
    fn names_are_made_readable() {
        assert_eq!(readable("Naruto__Shippuuden"), "Naruto Shippuuden");
        assert_eq!(readable("NARUTO-Shippuuden"), "NARUTO Shippuuden");
        assert_eq!(readable("Boruto_Ka%C3%AF"), "Boruto Kaï");
        assert_eq!(decoded("D.Gray-man_Ka%C3%AF", false), "D.Gray-man Kaï");
        assert_eq!(
            links("[https://fan-kai.fandom.com/fr/wiki/D.Gray-man_Ka%C3%AF D.Gray-man]"),
            vec!["D.Gray-man Kaï"]
        );
        assert_eq!(base("Naruto Yabai (Triggerforce)"), "Naruto Yabai");
        assert_eq!(
            qualifier("Naruto Yabai (Triggerforce)"),
            Some("Triggerforce")
        );
        assert_eq!(base("Black Lagoon Henshū"), "Black Lagoon Henshū");
        assert_eq!(qualifier("Black Lagoon Henshū"), None);
    }

    #[test]
    fn a_search_is_read_in_its_own_order() {
        let raw = json!({
            "batchcomplete": true,
            "query": { "pages": [
                { "pageid": 9, "title": "Second", "index": 2,
                  "revisions": [{ "slots": { "main": { "content": "b" } } }] },
                { "pageid": 3, "title": "First", "index": 1,
                  "revisions": [{ "slots": { "main": { "content": "a" } } }] },
                { "pageid": 5, "title": "No text", "index": 3 }
            ]}
        });
        let pages = results(&raw);
        assert_eq!(
            pages.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
            vec!["First", "Second"]
        );
        assert!(results(&json!({ "batchcomplete": true })).is_empty());
    }

    #[test]
    fn a_page_is_addressed_beside_the_api_it_was_read_from() {
        let client = FankaiWikiClient::new(
            reqwest::Client::new(),
            &config::FankaiWiki {
                upstream: "https://fan-kai.fandom.com/fr/api.php".into(),
                enabled: true,
            },
        );
        assert_eq!(
            client.page_url("Horimiya Kaï").as_deref(),
            Some("https://fan-kai.fandom.com/fr/wiki/Horimiya_Ka%C3%AF")
        );
        assert_eq!(
            client
                .page_url("Naruto Shippuden Yabai (Triggerforce)")
                .as_deref(),
            Some("https://fan-kai.fandom.com/fr/wiki/Naruto_Shippuden_Yabai_(Triggerforce)")
        );
        assert_eq!(
            client.page_url("What? #1").as_deref(),
            Some("https://fan-kai.fandom.com/fr/wiki/What%3F_%231")
        );
    }
}
