//! The two migration sets, applied and read back, and what they built compared.
//!
//! The schema is kept twice — `migrations/sqlite` and `migrations/postgres`,
//! so that each engine gets types of its own — and nothing but care keeps the
//! two the same. A column added to one set and forgotten in the other compiles,
//! passes every test that runs on SQLite, and fails on a PostgreSQL user's
//! first write. It is not enough that both apply, either: 0014 and 0021 widen
//! a CHECK, which SQLite can only do by building the table again and
//! PostgreSQL by altering it, and two routes to one shape are how one ends up
//! with two.
//!
//! So [`the_two_migration_sets_build_the_same_schema`] reads each schema back —
//! the tables; their columns, as the kind of value they hold, whether they may
//! be empty and what they default to; the keys, the indexes, the foreign keys,
//! the values a CHECK allows, the triggers — and compares what it read. What
//! the engines are meant to differ in is named in [`INTENDED`], each with its
//! reason; anything else that differs fails the test, and so does an entry
//! there that no longer differs, so the list cannot go stale.
//!
//! The kind of a column, not its type: SQLite's `INTEGER` is PostgreSQL's
//! `integer` or `bigint`, its `REAL` a `double precision`, and what a migration
//! says of them is a width and not a meaning. The test runs where there is a
//! PostgreSQL to read (see [`testing`]).

use std::collections::{BTreeMap, BTreeSet};

use sqlx::any::AnyRow;

use super::{Db, Dialect, RowExt, testing};

/// One thing true of a schema, written `<table>: <what>`. A set of them, so
/// that two schemas compare as sets and a difference reads as a list.
type Facts = BTreeSet<String>;

/// Where the engines differ on purpose, and why. A fact is covered by an entry
/// when it starts with `starting` and is on the side the entry says.
struct Intended {
    only_on: Dialect,
    starting: &'static str,
    /// An extension the difference exists only with: the entry is not looked
    /// for on a server without it.
    needs: Option<&'static str>,
    why: &'static str,
}

const FTS5: &str = "SQLite's search of titles is an FTS5 table, and FTS5 keeps five tables of \
                    its own behind it; PostgreSQL has none of them, and indexes the titles \
                    themselves (the trigram indexes below)";
const FTS5_TRIGGERS: &str = "what keeps the FTS5 table current — a work, an alternative title \
                             or an override written is that work's text built again; \
                             PostgreSQL reads the tables themselves, so there is nothing to keep";
const TRIGRAM: &str = "migration 0022: a trigram index (pg_trgm) of an expression the title \
                       search runs, where the extension can be created; SQLite's counterpart \
                       is the FTS5 table";

const INTENDED: &[Intended] = &[
    Intended {
        only_on: Dialect::Sqlite,
        starting: "media_search",
        needs: None,
        why: FTS5,
    },
    Intended {
        only_on: Dialect::Sqlite,
        starting: "media_item: trigger media_search_",
        needs: None,
        why: FTS5_TRIGGERS,
    },
    Intended {
        only_on: Dialect::Sqlite,
        starting: "media_alternative_title: trigger media_search_",
        needs: None,
        why: FTS5_TRIGGERS,
    },
    Intended {
        only_on: Dialect::Sqlite,
        starting: "media_override: trigger media_search_",
        needs: None,
        why: FTS5_TRIGGERS,
    },
    Intended {
        only_on: Dialect::Postgres,
        starting: "media_item: index idx_media_item_title_trgm ",
        needs: Some("pg_trgm"),
        why: TRIGRAM,
    },
    Intended {
        only_on: Dialect::Postgres,
        starting: "media_item: index idx_media_item_sort_title_trgm ",
        needs: Some("pg_trgm"),
        why: TRIGRAM,
    },
    Intended {
        only_on: Dialect::Postgres,
        starting: "media_item: index idx_media_item_slug_trgm ",
        needs: Some("pg_trgm"),
        why: TRIGRAM,
    },
    Intended {
        only_on: Dialect::Postgres,
        starting: "media_alternative_title: index idx_media_alternative_title_trgm ",
        needs: Some("pg_trgm"),
        why: TRIGRAM,
    },
    Intended {
        only_on: Dialect::Postgres,
        starting: "media_override: index idx_media_override_value_trgm ",
        needs: Some("pg_trgm"),
        why: TRIGRAM,
    },
];

#[tokio::test]
async fn the_two_migration_sets_build_the_same_schema() {
    let Some(postgres) = testing::postgres().await else {
        eprintln!("skipped: {} is not set", testing::VARIABLE);
        return;
    };
    let sqlite = testing::sqlite().await;

    let mut only_sqlite = read_sqlite(&sqlite).await;
    let mut only_postgres = read_postgres(&postgres).await;
    assert!(
        only_sqlite.len() > 200 && only_postgres.len() > 200,
        "the schemas were not read: {} facts from SQLite, {} from PostgreSQL",
        only_sqlite.len(),
        only_postgres.len()
    );

    // What each has and the other does not, then what that is meant to be.
    let both: Facts = only_sqlite.intersection(&only_postgres).cloned().collect();
    only_sqlite.retain(|fact| !both.contains(fact));
    only_postgres.retain(|fact| !both.contains(fact));

    let mut stale = Vec::new();
    for intended in INTENDED {
        if let Some(extension) = intended.needs
            && !extension_installed(&postgres, extension).await
        {
            continue;
        }

        let side = match intended.only_on {
            Dialect::Sqlite => &mut only_sqlite,
            Dialect::Postgres => &mut only_postgres,
        };
        let before = side.len();
        side.retain(|fact| !fact.starts_with(intended.starting));

        if side.len() == before {
            stale.push(format!(
                "{:?} no longer has anything starting {:?}: {}",
                intended.only_on, intended.starting, intended.why
            ));
        }
    }

    let listed = |facts: &Facts| {
        facts
            .iter()
            .map(|fact| format!("\n    {fact}"))
            .collect::<String>()
    };
    assert!(
        only_sqlite.is_empty() && only_postgres.is_empty() && stale.is_empty(),
        "the two migration sets do not build the same schema, and nothing in INTENDED says \
         they should not.\n  Only in SQLite's:{}\n  Only in PostgreSQL's:{}\n  Intended, and \
         gone:{}\n",
        listed(&only_sqlite),
        listed(&only_postgres),
        stale
            .iter()
            .map(|line| format!("\n    {line}"))
            .collect::<String>()
    );
}

#[tokio::test]
async fn what_sqlite_built_is_read_back_whole() {
    // Without a PostgreSQL to compare with, still enough to say the reading
    // is not empty-handed: a reader that found nothing would let the
    // comparison above pass against another that found nothing.
    let facts = read_sqlite(&testing::sqlite().await).await;

    for expected in [
        "media_item: table",
        "media_item: column title text not null",
        "media_item: primary key (id)",
        "media_item: index ix_media_item_title (title)",
        "media_item: index ux_media_item_kind_slug (kind, slug) unique",
        "media_item: index ix_media_item_refresh (refresh_after) partial",
        "media_item: index ix_media_item_browse (kind, is_enabled, popularity desc, title, is_adult)",
        "media_external_id: primary key (source, value)",
        "media_external_id: foreign key (media_id) references media_item (id) on delete cascade",
        "media_search: table",
    ] {
        assert!(
            facts.contains(expected),
            "{expected:?} was not read back; read {} facts",
            facts.len()
        );
    }

    // The values a source may be: the set is the migrations' to grow.
    assert!(
        facts.iter().any(
            |fact| fact.starts_with("media_external_id: check source in (")
                && fact.contains("fankai")
        ),
        "the allowed sources were not read back"
    );
}

// ─── reading SQLite ──────────────────────────────────────────────────────────

async fn read_sqlite(db: &Db) -> Facts {
    let mut facts = Facts::new();
    // The primary key of each table, in order: a foreign key that names no
    // column refers to it.
    let mut keys: BTreeMap<String, Vec<(i64, String)>> = BTreeMap::new();

    let columns = sqlx::query(db.sql(
        r#"SELECT m.name AS tbl, p.name AS col, p.type AS declared,
                  p."notnull" AS not_null, p.pk AS pk, p.dflt_value AS dflt
           FROM sqlite_master m, pragma_table_info(m.name) p
           WHERE m.type = 'table' AND substr(m.name, 1, 7) <> 'sqlite_'
             AND m.name <> '_sqlx_migrations'
           ORDER BY m.name, p.cid"#,
    ))
    .fetch_all(db.pool())
    .await
    .expect("the columns can be read");

    for row in &columns {
        let table = row.text("tbl").expect("a table");
        let column = row.text("col").expect("a column");
        let pk = row.big("pk").expect("a key position");
        let family = sqlite_family(&row.text("declared").expect("a declared type"));

        facts.insert(format!("{table}: table"));
        facts.insert(column_fact(
            &table,
            &column,
            &family,
            // A key may not be empty in PostgreSQL; in SQLite it may be, unless
            // said not to. Said or not, a key is not.
            row.big("not_null").expect("a flag") != 0 || pk > 0,
            row.opt_text("dflt").expect("a default").as_deref(),
        ));

        if pk > 0 {
            keys.entry(table).or_default().push((pk, column));
        }
    }

    let keys: BTreeMap<String, Vec<String>> = keys
        .into_iter()
        .map(|(table, mut columns)| {
            columns.sort();
            (table, columns.into_iter().map(|(_, c)| c).collect())
        })
        .collect();
    for (table, columns) in &keys {
        facts.insert(format!("{table}: primary key ({})", columns.join(", ")));
    }

    let indexes = sqlx::query(db.sql(
        r#"SELECT m.name AS tbl, il.name AS idx, il."unique" AS uniq, il.origin AS origin,
                  il.partial AS partial, ix.seqno AS seq, ix.name AS col, ix."desc" AS descending
           FROM sqlite_master m, pragma_index_list(m.name) il, pragma_index_xinfo(il.name) ix
           WHERE m.type = 'table' AND substr(m.name, 1, 7) <> 'sqlite_'
             AND m.name <> '_sqlx_migrations' AND ix.key = 1
           ORDER BY m.name, il.name, ix.seqno"#,
    ))
    .fetch_all(db.pool())
    .await
    .expect("the indexes can be read");
    index_facts(&indexes, false, &mut facts);

    let references = sqlx::query(db.sql(
        r#"SELECT m.name AS tbl, fk.id AS id, fk.seq AS seq, fk."table" AS ref_table,
                  fk."from" AS col, COALESCE(fk."to", '') AS ref_col, fk.on_delete AS on_delete
           FROM sqlite_master m, pragma_foreign_key_list(m.name) fk
           WHERE m.type = 'table' AND substr(m.name, 1, 7) <> 'sqlite_'
             AND m.name <> '_sqlx_migrations'
           ORDER BY m.name, fk.id, fk.seq"#,
    ))
    .fetch_all(db.pool())
    .await
    .expect("the foreign keys can be read");
    foreign_key_facts(&references, &keys, &mut facts);

    let declared = sqlx::query(db.sql(
        "SELECT name, tbl_name, type, sql FROM sqlite_master
         WHERE type IN ('table', 'trigger') AND name <> '_sqlx_migrations'",
    ))
    .fetch_all(db.pool())
    .await
    .expect("the definitions can be read");

    for row in &declared {
        let table = row.text("tbl_name").expect("a table");
        match row.text("type").expect("a type").as_str() {
            "trigger" => {
                facts.insert(format!(
                    "{table}: trigger {}",
                    row.text("name").expect("a name")
                ));
            }
            _ => {
                let sql = row
                    .opt_text("sql")
                    .expect("a definition")
                    .unwrap_or_default();
                for body in checks_in(&sql) {
                    facts.insert(check_fact(&table, &body));
                }
            }
        }
    }

    facts
}

/// What a declared type holds, by SQLite's own rules for its affinity.
fn sqlite_family(declared: &str) -> String {
    let declared = declared.to_ascii_uppercase();
    let has = |part: &str| declared.contains(part);

    if has("INT") {
        "int"
    } else if has("CHAR") || has("CLOB") || has("TEXT") {
        "text"
    } else if declared.is_empty() || has("BLOB") {
        "blob"
    } else if has("REAL") || has("FLOA") || has("DOUB") {
        "real"
    } else {
        "numeric"
    }
    .to_string()
}

// ─── reading PostgreSQL ──────────────────────────────────────────────────────

async fn read_postgres(db: &Db) -> Facts {
    let mut facts = Facts::new();

    let columns = sqlx::query(db.sql(
        "SELECT c.table_name::text AS tbl, c.column_name::text AS col,
                c.data_type::text AS declared,
                (CASE WHEN c.is_nullable = 'NO' THEN 1 ELSE 0 END)::bigint AS not_null,
                c.column_default::text AS dflt
         FROM information_schema.columns c
         JOIN information_schema.tables t
           ON t.table_schema = c.table_schema AND t.table_name = c.table_name
         WHERE c.table_schema = current_schema() AND t.table_type = 'BASE TABLE'
           AND c.table_name <> '_sqlx_migrations'
         ORDER BY c.table_name, c.ordinal_position",
    ))
    .fetch_all(db.pool())
    .await
    .expect("the columns can be read");

    for row in &columns {
        let table = row.text("tbl").expect("a table");
        facts.insert(format!("{table}: table"));
        facts.insert(column_fact(
            &table,
            &row.text("col").expect("a column"),
            &postgres_family(&row.text("declared").expect("a type")),
            row.big("not_null").expect("a flag") != 0,
            row.opt_text("dflt").expect("a default").as_deref(),
        ));
    }

    let indexes = sqlx::query(db.sql(
        r#"SELECT t.relname::text AS tbl, c.relname::text AS idx,
                  (CASE WHEN i.indisunique THEN 1 ELSE 0 END)::bigint AS uniq,
                  (CASE WHEN i.indisprimary THEN 'pk' WHEN con.oid IS NOT NULL THEN 'u'
                        ELSE 'c' END)::text AS origin,
                  (CASE WHEN i.indpred IS NOT NULL THEN 1 ELSE 0 END)::bigint AS partial,
                  (k.ord - 1)::bigint AS seq,
                  (CASE WHEN k.attnum = 0 THEN '' ELSE a.attname::text END)::text AS col,
                  (CASE WHEN (i.indoption[(k.ord - 1)::int] & 1) = 1 THEN 1 ELSE 0 END)::bigint
                      AS descending
           FROM pg_index i
           JOIN pg_class c ON c.oid = i.indexrelid
           JOIN pg_class t ON t.oid = i.indrelid
           CROSS JOIN LATERAL unnest(i.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord)
           LEFT JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum
           LEFT JOIN pg_constraint con ON con.conindid = i.indexrelid AND con.contype IN ('u', 'p')
           WHERE t.relnamespace = current_schema()::regnamespace AND t.relname <> '_sqlx_migrations'
             AND k.ord <= i.indnkeyatts
           ORDER BY t.relname, c.relname, k.ord"#,
    ))
    .fetch_all(db.pool())
    .await
    .expect("the indexes can be read");
    index_facts(&indexes, true, &mut facts);

    let references = sqlx::query(db.sql(
        "SELECT t.relname::text AS tbl, con.oid::bigint AS id, (k.ord - 1)::bigint AS seq,
                rt.relname::text AS ref_table, a.attname::text AS col,
                ra.attname::text AS ref_col, con.confdeltype::text AS on_delete
         FROM pg_constraint con
         JOIN pg_class t ON t.oid = con.conrelid
         JOIN pg_class rt ON rt.oid = con.confrelid
         CROSS JOIN LATERAL unnest(con.conkey, con.confkey) WITH ORDINALITY AS k(attnum, refattnum, ord)
         JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum
         JOIN pg_attribute ra ON ra.attrelid = con.confrelid AND ra.attnum = k.refattnum
         WHERE con.contype = 'f' AND t.relnamespace = current_schema()::regnamespace
         ORDER BY t.relname, con.oid, k.ord",
    ))
    .fetch_all(db.pool())
    .await
    .expect("the foreign keys can be read");
    foreign_key_facts(&references, &BTreeMap::new(), &mut facts);

    let checks = sqlx::query(db.sql(
        "SELECT t.relname::text AS tbl, pg_get_constraintdef(con.oid)::text AS def
         FROM pg_constraint con JOIN pg_class t ON t.oid = con.conrelid
         WHERE con.contype = 'c' AND t.relnamespace = current_schema()::regnamespace",
    ))
    .fetch_all(db.pool())
    .await
    .expect("the checks can be read");
    for row in &checks {
        let table = row.text("tbl").expect("a table");
        for body in checks_in(&row.text("def").expect("a definition")) {
            facts.insert(check_fact(&table, &body));
        }
    }

    let triggers = sqlx::query(db.sql(
        "SELECT g.tgname::text AS name, c.relname::text AS tbl
         FROM pg_trigger g JOIN pg_class c ON c.oid = g.tgrelid
         WHERE NOT g.tgisinternal AND c.relnamespace = current_schema()::regnamespace",
    ))
    .fetch_all(db.pool())
    .await
    .expect("the triggers can be read");
    for row in &triggers {
        facts.insert(format!(
            "{}: trigger {}",
            row.text("tbl").expect("a table"),
            row.text("name").expect("a name")
        ));
    }

    facts
}

/// What a column's type holds, by the name PostgreSQL gives it. A type nobody
/// asked for is left as it is, so that it shows up as a difference.
fn postgres_family(data_type: &str) -> String {
    match data_type {
        "smallint" | "integer" | "bigint" => "int",
        "real" | "double precision" => "real",
        "text" | "character varying" | "character" => "text",
        "bytea" => "blob",
        other => other,
    }
    .to_string()
}

async fn extension_installed(db: &Db, extension: &str) -> bool {
    let found = sqlx::query(db.sql("SELECT COUNT(*) AS n FROM pg_extension WHERE extname = ?"))
        .bind(extension.to_string())
        .fetch_one(db.pool())
        .await
        .expect("the extensions can be listed");
    found.big("n").expect("a count") > 0
}

// ─── what both readings share ────────────────────────────────────────────────

fn column_fact(
    table: &str,
    column: &str,
    family: &str,
    not_null: bool,
    default: Option<&str>,
) -> String {
    format!(
        "{table}: column {column} {family}{}{}",
        if not_null { " not null" } else { "" },
        default
            .map(|text| format!(" default {}", plain_default(text, family)))
            .unwrap_or_default()
    )
}

/// A default as both engines would write it: PostgreSQL adds the type a
/// literal was cast to, and puts a negative number in quotes to cast it.
fn plain_default(default: &str, family: &str) -> String {
    let default = default.trim();
    let uncast = match default.rfind("::") {
        Some(at) if !default[at..].contains('\'') => default[..at].trim(),
        _ => default,
    };
    let unwrapped = uncast.strip_prefix('(').and_then(|s| s.strip_suffix(')'));
    let uncast = unwrapped.unwrap_or(uncast);

    match (
        family,
        uncast.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')),
    ) {
        ("int" | "real", Some(number)) => number.to_string(),
        _ => uncast.to_string(),
    }
}

/// The keys other than the primary one, and the indexes, from rows of
/// `tbl, idx, uniq, origin, partial, seq, col, descending` — the same columns
/// whichever engine they came from. `keys` says whether the primary keys are
/// to be had from here: PostgreSQL builds an index for each, SQLite does not
/// always, so SQLite's are read from its columns instead.
fn index_facts(rows: &[AnyRow], keys: bool, facts: &mut Facts) {
    struct Index {
        origin: String,
        unique: bool,
        partial: bool,
        columns: Vec<(i64, String)>,
    }

    let mut indexes: BTreeMap<(String, String), Index> = BTreeMap::new();
    for row in rows {
        let index = indexes
            .entry((
                row.text("tbl").expect("a table"),
                row.text("idx").expect("an index"),
            ))
            .or_insert_with(|| Index {
                origin: row.text("origin").expect("an origin"),
                unique: row.big("uniq").expect("a flag") != 0,
                partial: row.big("partial").expect("a flag") != 0,
                columns: Vec::new(),
            });

        // An expression has no column to name.
        let column = row
            .opt_text("col")
            .expect("a column")
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "expr".to_string());
        let descending = row.big("descending").expect("a flag") != 0;
        index.columns.push((
            row.big("seq").expect("a position"),
            if descending {
                format!("{column} desc")
            } else {
                column
            },
        ));
    }

    for ((table, name), mut index) in indexes {
        index.columns.sort();
        let columns = index
            .columns
            .into_iter()
            .map(|(_, column)| column)
            .collect::<Vec<_>>()
            .join(", ");

        // Their names are the engines' own (`sqlite_autoindex_…`,
        // `…_key`): what is a fact is the columns they cover.
        match index.origin.as_str() {
            "pk" if keys => {
                facts.insert(format!("{table}: primary key ({columns})"));
            }
            "pk" => {}
            "u" => {
                facts.insert(format!("{table}: unique ({columns})"));
            }
            _ => {
                facts.insert(format!(
                    "{table}: index {name} ({columns}){}{}",
                    if index.unique { " unique" } else { "" },
                    if index.partial { " partial" } else { "" }
                ));
            }
        }
    }
}

/// The foreign keys, from rows of
/// `tbl, id, seq, ref_table, col, ref_col, on_delete`. A key written without
/// the columns it refers to refers to the other table's primary key.
fn foreign_key_facts(rows: &[AnyRow], keys: &BTreeMap<String, Vec<String>>, facts: &mut Facts) {
    // (table, key) → what it refers to, and what becomes of the row when that goes.
    type Key = (String, Vec<(i64, String, String)>, String);
    let mut found: BTreeMap<(String, i64), Key> = BTreeMap::new();

    for row in rows {
        let table = row.text("tbl").expect("a table");
        let referred = row.text("ref_table").expect("a table");
        let seq = row.big("seq").expect("a position");
        let mut target = row.text("ref_col").expect("a column");
        if target.is_empty() {
            target = keys
                .get(&referred)
                .and_then(|columns| columns.get(usize::try_from(seq).unwrap_or_default()))
                .cloned()
                .unwrap_or_default();
        }

        found
            .entry((table, row.big("id").expect("an id")))
            .or_insert_with(|| {
                (
                    referred,
                    Vec::new(),
                    on_delete(&row.text("on_delete").expect("an action")),
                )
            })
            .1
            .push((seq, row.text("col").expect("a column"), target));
    }

    for ((table, _), (referred, mut columns, action)) in found {
        columns.sort();
        let from = columns
            .iter()
            .map(|(_, c, _)| c.as_str())
            .collect::<Vec<_>>();
        let to = columns
            .iter()
            .map(|(_, _, c)| c.as_str())
            .collect::<Vec<_>>();
        facts.insert(format!(
            "{table}: foreign key ({}) references {referred} ({}) on delete {action}",
            from.join(", "),
            to.join(", ")
        ));
    }
}

/// What becomes of a row whose parent goes: as SQLite spells it, or as
/// PostgreSQL codes it.
fn on_delete(action: &str) -> String {
    match action {
        "a" | "NO ACTION" => "no action",
        "r" | "RESTRICT" => "restrict",
        "c" | "CASCADE" => "cascade",
        "n" | "SET NULL" => "set null",
        "d" | "SET DEFAULT" => "set default",
        other => other,
    }
    .to_string()
}

/// A CHECK as the values it allows a column: SQLite keeps the text it was
/// written in, PostgreSQL gives it back reworked (`= ANY (ARRAY['a'::text, …])`
/// for `IN ('a', …)`), and what both hold is the column and the set.
fn check_fact(table: &str, body: &str) -> String {
    let column: String = body
        .trim_start_matches(|c: char| !(c.is_alphabetic() || c == '_'))
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();

    format!(
        "{table}: check {column} in ({})",
        literals_in(body).into_iter().collect::<Vec<_>>().join(", ")
    )
}

/// The single-quoted strings in some SQL, a doubled quote being one inside.
fn literals_in(sql: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut chars = sql.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\'' {
            continue;
        }
        let mut literal = String::new();
        while let Some(next) = chars.next() {
            if next != '\'' {
                literal.push(next);
            } else if chars.peek() == Some(&'\'') {
                chars.next();
                literal.push('\'');
            } else {
                break;
            }
        }
        found.insert(literal);
    }

    found
}

/// What is inside the parentheses of each `CHECK` in some SQL — a table as it
/// was written, or a constraint as PostgreSQL gives it back.
fn checks_in(sql: &str) -> Vec<String> {
    let text = without_comments(sql);
    // The same length, so that a position in one is a position in the other.
    let upper = text.to_ascii_uppercase();
    let bytes = text.as_bytes();

    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = upper[from..].find("CHECK") {
        let start = from + at;
        from = start + "CHECK".len();

        // A word of its own, and followed by what it checks.
        let alone =
            start == 0 || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
        let after = text[from..].trim_start();
        if !alone || !after.starts_with('(') {
            continue;
        }

        let open = text.len() - after.len();
        let mut depth = 0usize;
        let mut quoted = false;
        for (offset, c) in text[open..].char_indices() {
            match c {
                '\'' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted => {
                    depth -= 1;
                    if depth == 0 {
                        found.push(text[open + 1..open + offset].to_string());
                        from = open + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    found
}

/// SQL without its comments, so that a CHECK mentioned in one is not read as a
/// constraint. SQLite keeps the text of a table as it was written, comments
/// included.
fn without_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut quoted = false;

    while let Some(c) = chars.next() {
        if quoted {
            quoted = c != '\'';
            out.push(c);
            continue;
        }

        match c {
            '\'' => {
                quoted = true;
                out.push(c);
            }
            '-' if chars.peek() == Some(&'-') => {
                while chars.peek().is_some_and(|next| *next != '\n') {
                    chars.next();
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_is_the_same_fact_however_the_engine_spells_it() {
        let sqlite = "CREATE TABLE media_item (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL CHECK (kind IN ('series', 'movie')), -- a check (of kinds)
            note TEXT /* CHECK (note IN ('no')) */
        )";
        let postgres = "CHECK ((kind = ANY (ARRAY['series'::text, 'movie'::text])))";

        let from_sqlite: Vec<_> = checks_in(sqlite)
            .iter()
            .map(|body| check_fact("media_item", body))
            .collect();
        let from_postgres: Vec<_> = checks_in(postgres)
            .iter()
            .map(|body| check_fact("media_item", body))
            .collect();

        assert_eq!(
            from_sqlite,
            ["media_item: check kind in (movie, series)"],
            "a comment is not a constraint"
        );
        assert_eq!(from_sqlite, from_postgres);
    }

    #[test]
    fn a_quote_inside_a_literal_and_a_parenthesis_inside_one_do_not_end_the_check() {
        let sql = "CHECK (label IN ('it''s (odd', 'plain'))";
        assert_eq!(
            checks_in(sql)
                .iter()
                .map(|body| check_fact("t", body))
                .collect::<Vec<_>>(),
            ["t: check label in (it's (odd, plain)"]
        );
    }

    #[test]
    fn a_declared_type_is_read_by_what_it_holds() {
        for (declared, family) in [
            ("INTEGER", "int"),
            ("BIGINT", "int"),
            ("TEXT", "text"),
            ("VARCHAR(20)", "text"),
            ("REAL", "real"),
            ("BLOB", "blob"),
        ] {
            assert_eq!(sqlite_family(declared), family, "{declared}");
        }
        for (data_type, family) in [
            ("integer", "int"),
            ("bigint", "int"),
            ("text", "text"),
            ("double precision", "real"),
            ("bytea", "blob"),
            ("jsonb", "jsonb"),
        ] {
            assert_eq!(postgres_family(data_type), family, "{data_type}");
        }
    }

    #[test]
    fn a_default_reads_the_same_whichever_engine_wrote_it() {
        for (sqlite, postgres, family) in [
            ("0", "0", "int"),
            ("'[]'", "'[]'::text", "text"),
            ("''", "''::text", "text"),
            ("-1", "'-1'::integer", "int"),
            ("1.5", "'1.5'::double precision", "real"),
            ("'a::b'", "'a::b'::text", "text"),
        ] {
            assert_eq!(
                plain_default(sqlite, family),
                plain_default(postgres, family),
                "{sqlite} / {postgres}"
            );
        }
    }
}
