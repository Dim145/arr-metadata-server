//! Genres as things of their own: a slug that is the same in every language,
//! and a name in each language the genre is known in.
//!
//! The providers name a genre in the language they are asked in, and some in
//! their own words whatever they are asked: TMDB says "Drama" in English and
//! "Drame" in French, AniList "Sci-Fi" where TMDB says "Science Fiction".
//! Stored as they came, one catalogue held both "Drama" and "Drame" once the
//! server's language had changed, and every list offered the genre twice.
//!
//! So a genre is listed under its slug — `drama`, `science-fiction` — which
//! every name it goes by leads to: the genres the server knows (TMDB's, in
//! the two languages the interface speaks, TheTVDB's, AniList's,
//! MyAnimeList's and Fankai's) by the table below, any other by its name made
//! into a slug. The works' `listed_genres` hold the slugs (`Listed::of`); the
//! `genre` rows, filled as works are listed ([`learn`]), keep what each slug
//! is called, so a page and a count can name it in the reader's language.
//!
//! A work's own `genres` stay as its providers gave them: they are what
//! Sonarr, Radarr, Plex and the NFO files are given.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{Arguments, any::AnyArguments};
use utoipa::ToSchema;

use crate::db::{Db, RowExt, new_id, now};

/// A genre the server knows: its slug, what it is called in English and in
/// French, and the other names a source gives it.
#[derive(Debug)]
pub struct Known {
    pub slug: &'static str,
    pub en: &'static str,
    pub fr: &'static str,
    /// Another provider's word for it, or a French source's: `Sci-Fi`,
    /// `Policier`.
    pub aliases: &'static [&'static str],
}

const fn known(
    slug: &'static str,
    en: &'static str,
    fr: &'static str,
    aliases: &'static [&'static str],
) -> Known {
    Known {
        slug,
        en,
        fr,
        aliases,
    }
}

/// The genres the server knows by every name its sources give them.
///
/// TMDB's film and television genres in English and in French (its three
/// combined ones are taken apart before they get here: see
/// [`crate::domain::genre_parts`]), TheTVDB's, AniList's, MyAnimeList's
/// main ones, and the French words Fankai uses. Two names of one thing are
/// one genre: TheTVDB's "Children" is TMDB's "Kids", AniList's "Sports"
/// TheTVDB's "Sport". The French names are the interface's own
/// (`frontend/src/lib/labels.ts`).
pub const KNOWN: &[Known] = &[
    known("action", "Action", "Action", &[]),
    known("adventure", "Adventure", "Aventure", &[]),
    known("animation", "Animation", "Animation", &[]),
    known("anime", "Anime", "Anime", &[]),
    known("comedy", "Comedy", "Comédie", &[]),
    known("crime", "Crime", "Crime", &["Policier"]),
    known("documentary", "Documentary", "Documentaire", &[]),
    known("drama", "Drama", "Drame", &[]),
    known("family", "Family", "Familial", &["Famille"]),
    known("fantasy", "Fantasy", "Fantastique", &[]),
    known("history", "History", "Histoire", &["Historical"]),
    known("horror", "Horror", "Horreur", &[]),
    known("music", "Music", "Musique", &[]),
    known("musical", "Musical", "Comédie musicale", &[]),
    known("mystery", "Mystery", "Mystère", &[]),
    known("romance", "Romance", "Romance", &[]),
    known(
        "science-fiction",
        "Science Fiction",
        "Science-fiction",
        &["Sci-Fi", "SciFi", "SF"],
    ),
    known("tv-movie", "TV Movie", "Téléfilm", &[]),
    known("thriller", "Thriller", "Thriller", &[]),
    known("war", "War", "Guerre", &[]),
    known("politics", "Politics", "Politique", &[]),
    known("western", "Western", "Western", &[]),
    known("kids", "Kids", "Jeunesse", &["Children", "Enfants"]),
    known("news", "News", "Actualités", &[]),
    known("reality", "Reality", "Téléréalité", &["Reality TV"]),
    known("soap", "Soap", "Feuilleton", &["Soap Opera"]),
    known("talk-show", "Talk Show", "Talk-show", &["Talk"]),
    known("food", "Food", "Cuisine", &["Nourriture"]),
    known("game-show", "Game Show", "Jeu télévisé", &[]),
    known(
        "home-and-garden",
        "Home and Garden",
        "Maison et jardin",
        &[],
    ),
    known("indie", "Indie", "Indépendant", &[]),
    known("martial-arts", "Martial Arts", "Arts martiaux", &[]),
    known("mini-series", "Mini-Series", "Mini-série", &["Miniseries"]),
    known("podcast", "Podcast", "Podcast", &[]),
    known("sport", "Sport", "Sport", &["Sports"]),
    known("suspense", "Suspense", "Suspense", &["Suspens"]),
    known("travel", "Travel", "Voyage", &[]),
    known("awards-show", "Awards Show", "Remise de prix", &[]),
    known(
        "special-interest",
        "Special Interest",
        "Centres d’intérêt",
        &[],
    ),
    known("ecchi", "Ecchi", "Ecchi", &[]),
    known("hentai", "Hentai", "Hentai", &[]),
    known(
        "mahou-shoujo",
        "Mahou Shoujo",
        "Magical girl",
        &["Mahou Shojo"],
    ),
    known("mecha", "Mecha", "Mecha", &[]),
    known("psychological", "Psychological", "Psychologique", &[]),
    known("slice-of-life", "Slice of Life", "Tranche de vie", &[]),
    known("supernatural", "Supernatural", "Surnaturel", &[]),
    known("avant-garde", "Avant Garde", "Avant-garde", &[]),
    known("award-winning", "Award Winning", "Primé", &[]),
    known("boys-love", "Boys Love", "Boys’ love", &[]),
    known("girls-love", "Girls Love", "Girls’ love", &[]),
    known("gourmet", "Gourmet", "Gastronomie", &[]),
    known("erotica", "Erotica", "Érotique", &[]),
];

/// Every name of [`KNOWN`], as a slug, and the genre it is a name of.
static BY_NAME: LazyLock<HashMap<String, &'static Known>> = LazyLock::new(|| {
    let mut by_name = HashMap::new();
    for genre in KNOWN {
        for name in [genre.slug, genre.en, genre.fr].iter().chain(genre.aliases) {
            by_name.entry(slug::slugify(name)).or_insert(genre);
        }
    }
    by_name
});

/// The slug a genre is listed under, from any name it goes by or from its
/// slug: `Drama`, `Drame` and `drama` are all `drama`, and so is ` DRAMA `.
/// A genre the server does not know is its name made into a slug —
/// `Sword & Sorcery` is `sword-sorcery`, `Komödie` is `komodie` — which
/// leads back to itself. Nothing for a name that says nothing.
///
/// A name with no letter an address can carry, which the slug's
/// transliteration does not cover, is its own slug in lower case, so that
/// it is still a genre of its own rather than none.
pub fn slug_of(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let folded = slug::slugify(name);
    if folded.is_empty() {
        return Some(name.to_lowercase());
    }
    Some(match BY_NAME.get(&folded) {
        Some(genre) => genre.slug.to_string(),
        None => folded,
    })
}

/// The genre the server knows by this slug, if it knows one.
pub fn known_slug(slug: &str) -> Option<&'static Known> {
    BY_NAME
        .get(slug)
        .filter(|genre| genre.slug == slug)
        .copied()
}

/// The names a slug is known by in the table, whatever the database holds:
/// what a list filtering by the slug also looks for in a work not listed
/// again since its genres were names.
pub fn known_names(slug: &str) -> Vec<&'static str> {
    match known_slug(slug) {
        Some(genre) => {
            let mut names = vec![genre.en];
            if genre.fr != genre.en {
                names.push(genre.fr);
            }
            names
        }
        None => Vec::new(),
    }
}

/// A work's genres as it is listed under them: each the slug of one of its
/// genres, or of one part of a combined one, with the name it was given;
/// each slug once, in the order the work gives them.
pub fn of_names<S: AsRef<str>>(names: &[S]) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    names
        .iter()
        // A NUL, which a lock can hold, is no part of a name, and PostgreSQL
        // would refuse the name it is filed under.
        .flat_map(|name| crate::domain::genre_parts(&name.as_ref().replace('\0', "")))
        .filter_map(|part| {
            let slug = slug_of(&part)?;
            seen.insert(slug.clone()).then_some((slug, part))
        })
        .collect()
}

/// A genre: its slug, and its name in each language it is known in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Genre {
    /// What it is listed and addressed by: `drama`.
    pub slug: String,
    /// Its name by language: `en`, `fr`, and `und` for the name a source gave
    /// it by where nothing says which language that is.
    pub names: BTreeMap<String, String>,
}

impl Genre {
    /// What a genre the server knows is called, from the table alone.
    fn of_known(genre: &Known) -> Self {
        let mut names = BTreeMap::new();
        names.insert("en".to_string(), genre.en.to_string());
        names.insert("fr".to_string(), genre.fr.to_string());
        Self {
            slug: genre.slug.to_string(),
            names,
        }
    }

    /// Its name in `language` — two letters, `fr` — where it has one; else
    /// in English, else as a source gave it, else in any language, else its
    /// slug.
    pub fn name(&self, language: Option<&str>) -> String {
        let language = language.map(language_of);
        language
            .as_deref()
            .and_then(|l| self.names.get(l))
            .or_else(|| self.names.get("en"))
            .or_else(|| self.names.get("und"))
            .or_else(|| self.names.values().next())
            .cloned()
            .unwrap_or_else(|| self.slug.clone())
    }

    /// The name a list's filter and its counts give it: its English one,
    /// which the interface translates, or the one a source gave it by.
    pub fn listed_name(&self) -> String {
        self.name(Some("en"))
    }
}

/// A language as genre names are filed under it: its first part, two or three
/// letters in lower case — `fr` for `fr-FR`.
pub fn language_of(tag: &str) -> String {
    tag.split(['-', '_'])
        .next()
        .unwrap_or(tag)
        .trim()
        .to_ascii_lowercase()
}

/// How many slugs one statement binds.
const CHUNK: usize = 200;

/// The names filed under these slugs, by slug and language.
async fn filed(db: &Db, slugs: &[String]) -> Result<HashMap<String, BTreeMap<String, String>>> {
    let mut found: HashMap<String, BTreeMap<String, String>> = HashMap::new();
    for chunk in slugs.chunks(CHUNK) {
        let sql = format!(
            "SELECT g.slug, n.language, n.name FROM genre g
               JOIN genre_name n ON n.genre_id = g.id
              WHERE g.slug IN ({})",
            vec!["?"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for slug in chunk {
            args.add(slug.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let rows = sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?;
        for row in &rows {
            found
                .entry(row.text("slug")?)
                .or_default()
                .insert(row.text("language")?, row.text("name")?);
        }
    }
    Ok(found)
}

/// The genres these slugs are, as far as the table and the database know
/// them: the names the server knows over any filed — they are the ones the
/// interface speaks — and the database's for the rest. A slug neither knows
/// is left out.
pub async fn many(db: &Db, slugs: &[String]) -> Result<HashMap<String, Genre>> {
    let mut asked: Vec<String> = slugs.to_vec();
    asked.sort();
    asked.dedup();
    let unknown: Vec<String> = asked
        .iter()
        .filter(|slug| known_slug(slug).is_none())
        .cloned()
        .collect();
    let mut filed = if unknown.is_empty() {
        HashMap::new()
    } else {
        filed(db, &unknown).await?
    };

    let mut genres = HashMap::new();
    for slug in asked {
        let genre = match known_slug(&slug) {
            Some(known) => Genre::of_known(known),
            None => match filed.remove(&slug) {
                Some(names) => Genre {
                    slug: slug.clone(),
                    names,
                },
                None => continue,
            },
        };
        genres.insert(slug, genre);
    }
    Ok(genres)
}

/// A genre by its slug or by any name it goes by, as [`many`] reads it.
/// Nothing for one neither the table nor the database knows.
pub async fn find(db: &Db, asked: &str) -> Result<Option<Genre>> {
    let Some(slug) = slug_of(asked) else {
        return Ok(None);
    };
    Ok(many(db, std::slice::from_ref(&slug)).await?.remove(&slug))
}

/// File the genres a work was just listed under that are not filed yet: one
/// the server knows with its names in English and in French, any other with
/// the name it was given, as `und`. `seen` is [`of_names`]'s: a slug and the
/// name that led to it. Returns how many were filed.
///
/// Two listings filing the same genre at once file it once: the second finds
/// the slug taken and leaves it.
pub async fn learn(db: &Db, seen: &[(String, String)]) -> Result<u64> {
    let mut first: BTreeMap<&str, &str> = BTreeMap::new();
    for (slug, name) in seen {
        first.entry(slug.as_str()).or_insert(name.as_str());
    }
    if first.is_empty() {
        return Ok(0);
    }

    let asked: Vec<String> = first.keys().map(|s| s.to_string()).collect();
    let mut already: HashSet<String> = HashSet::new();
    for chunk in asked.chunks(CHUNK) {
        let sql = format!(
            "SELECT slug FROM genre WHERE slug IN ({})",
            vec!["?"; chunk.len()].join(", ")
        );
        let mut args = AnyArguments::default();
        for slug in chunk {
            args.add(slug.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        for row in sqlx::query_with(db.sql(&sql), args)
            .fetch_all(db.pool())
            .await?
        {
            already.insert(row.text("slug")?);
        }
    }
    let new: Vec<(&str, &str)> = first
        .into_iter()
        .filter(|(slug, _)| !already.contains(*slug))
        .collect();
    if new.is_empty() {
        return Ok(0);
    }

    let mut tx = db.begin_write().await?;
    let at = now();
    let mut filed = 0;
    for (slug, name) in new {
        let id = new_id();
        let written = sqlx::query(db.sql(
            "INSERT INTO genre (id, slug, created_at, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (slug) DO NOTHING",
        ))
        .bind(&id)
        .bind(slug)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("failed to file the genre {slug:?}"))?;
        if written.rows_affected() == 0 {
            continue;
        }

        let names: Vec<(&str, &str)> = match known_slug(slug) {
            Some(genre) => vec![("en", genre.en), ("fr", genre.fr)],
            None => vec![("und", name)],
        };
        for (language, name) in names {
            sqlx::query(db.sql(
                "INSERT INTO genre_name (genre_id, language, name) VALUES (?, ?, ?)
                 ON CONFLICT (genre_id, language) DO NOTHING",
            ))
            .bind(&id)
            .bind(language)
            .bind(name)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("failed to name the genre {slug:?}"))?;
        }
        filed += 1;
    }
    tx.commit()
        .await
        .context("failed to commit the genres filed")?;
    Ok(filed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drama_and_drame_are_one_genre() {
        for name in ["Drama", "Drame", "drama", " DRAMA ", "drame"] {
            assert_eq!(slug_of(name).as_deref(), Some("drama"), "{name}");
        }
        // TMDB's French film genre, TheTVDB's and AniList's words for one
        // thing, and a slug written as an address would have it.
        for name in [
            "Science Fiction",
            "Science-Fiction",
            "Sci-Fi",
            "science-fiction",
        ] {
            assert_eq!(slug_of(name).as_deref(), Some("science-fiction"), "{name}");
        }
        assert_eq!(slug_of("Comédie").as_deref(), Some("comedy"));
        assert_eq!(slug_of("Children").as_deref(), Some("kids"));
        assert_eq!(slug_of("Policier").as_deref(), Some("crime"));
        assert_eq!(slug_of("Comédie musicale").as_deref(), Some("musical"));
        assert_eq!(slug_of("Téléfilm").as_deref(), Some("tv-movie"));
    }

    #[test]
    fn a_genre_the_server_does_not_know_is_its_name_as_a_slug() {
        assert_eq!(slug_of("Sword & Sorcery").as_deref(), Some("sword-sorcery"));
        assert_eq!(slug_of("Komödie").as_deref(), Some("komodie"));
        assert_eq!(slug_of("   "), None);
        assert_eq!(slug_of(""), None);
    }

    #[test]
    fn a_slug_leads_back_to_itself() {
        for genre in KNOWN {
            assert_eq!(slug_of(genre.slug).as_deref(), Some(genre.slug));
            assert_eq!(slug::slugify(genre.slug), genre.slug, "{}", genre.slug);
        }
        for name in ["Sword & Sorcery", "Komödie", "Isekai", "Iyashikei"] {
            let slug = slug_of(name).unwrap();
            assert_eq!(slug_of(&slug), Some(slug.clone()), "{name}");
        }
    }

    #[test]
    fn no_name_leads_to_two_genres() {
        let mut seen: HashMap<String, &str> = HashMap::new();
        for genre in KNOWN {
            for name in [genre.slug, genre.en, genre.fr].iter().chain(genre.aliases) {
                let key = slug::slugify(name);
                if let Some(other) = seen.insert(key.clone(), genre.slug) {
                    assert_eq!(other, genre.slug, "{name} names {other} and {}", genre.slug);
                }
            }
        }
    }

    #[test]
    fn a_work_is_listed_under_each_genre_once_its_combined_ones_taken_apart() {
        let listed = of_names(&[
            "Sci-Fi & Fantasy",
            "Fantastique",
            "Science Fiction",
            "Drame",
            "Drama",
        ]);
        assert_eq!(
            listed,
            [
                ("science-fiction".to_string(), "Science Fiction".to_string()),
                ("fantasy".to_string(), "Fantasy".to_string()),
                ("drama".to_string(), "Drame".to_string()),
            ]
        );
    }

    #[test]
    fn a_nul_a_lock_holds_is_no_part_of_a_genre_s_name() {
        assert_eq!(
            of_names(&["Kom\0ödie"]),
            [("komodie".to_string(), "Komödie".to_string())]
        );
    }

    #[test]
    fn a_genre_is_named_in_the_reader_s_language_or_the_nearest_one() {
        let drama = Genre::of_known(known_slug("drama").unwrap());
        assert_eq!(drama.name(Some("fr")), "Drame");
        assert_eq!(drama.name(Some("fr-FR")), "Drame");
        assert_eq!(drama.name(Some("de")), "Drama");
        assert_eq!(drama.name(None), "Drama");
        assert_eq!(drama.listed_name(), "Drama");

        let mut names = BTreeMap::new();
        names.insert("und".to_string(), "Komödie".to_string());
        let learned = Genre {
            slug: "komodie".into(),
            names,
        };
        assert_eq!(learned.name(Some("fr")), "Komödie");
        assert_eq!(learned.listed_name(), "Komödie");
        let bare = Genre {
            slug: "nameless".into(),
            names: BTreeMap::new(),
        };
        assert_eq!(bare.name(Some("en")), "nameless");
    }

    #[tokio::test]
    async fn the_genres_of_a_listing_are_filed_once_with_their_names() {
        let db = crate::db::testing::db().await;
        let seen = of_names(&["Drame", "Komödie", "Sword & Sorcery"]);
        assert_eq!(learn(&db, &seen).await.unwrap(), 3);
        // Listed again, nothing new.
        assert_eq!(learn(&db, &seen).await.unwrap(), 0);
        assert_eq!(learn(&db, &[]).await.unwrap(), 0);

        let drama = find(&db, "Drama").await.unwrap().expect("known");
        assert_eq!(drama.slug, "drama");
        assert_eq!(drama.names["fr"], "Drame");
        assert_eq!(drama.names["en"], "Drama");

        let learned = find(&db, "komodie").await.unwrap().expect("filed");
        assert_eq!(
            learned.names.get("und").map(String::as_str),
            Some("Komödie")
        );
        assert_eq!(learned.name(Some("fr")), "Komödie");

        // A genre the server knows is found before anything is filed; one
        // it does not, only once it has been.
        assert!(find(&db, "western").await.unwrap().is_some());
        assert!(find(&db, "isekai").await.unwrap().is_none());

        let found = many(
            &db,
            &[
                "drama".into(),
                "komodie".into(),
                "isekai".into(),
                "drama".into(),
            ],
        )
        .await
        .unwrap();
        let mut slugs: Vec<&str> = found.keys().map(String::as_str).collect();
        slugs.sort_unstable();
        assert_eq!(slugs, ["drama", "komodie"]);
    }
}
