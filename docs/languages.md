# Languages

Entries are stored in whatever `AMS_TMDB_LANGUAGE` is set to. **If you want your
stack in French, set that** — it is what every client gets by default, and it is
almost certainly the setting you want:

```bash
AMS_TMDB_LANGUAGE=fr-FR
```

The variable gives the setting `tmdb.language` its first value, at the first start. After
that, the language of the answers is changed under **Settings › Answering**, without a
restart, and it can be set for one key or one address rule too — which is how one Radarr
answers in French while everything else answers in English. See
[`tmdb.language`](settings.md#tmdblanguage).

Any *other* language is fetched the first time something asks for it and kept
from then on:

```bash
curl 'http://localhost:8080/v1/tvdb/shows/fr/81189'       # the Skyhook form
curl 'http://localhost:8080/api/v1/items/{id}?language=ja'
```

One caveat worth knowing before you rely on the per-request form: **Sonarr v4
does not use it.** Its request builder sets the language segment to `en` once
and never changes it, so every request from a Sonarr v4 arrives as English
whatever its own UI language says, and `AMS_TMDB_LANGUAGE` — the setting
`tmdb.language`, which it seeds — is the knob for it.
**Sonarr v5 does.** It asks `/v2/tvdb/shows/{language}/{id}/{seasonType}` in
the language a person set for the series, as three letters (`eng`, `fra`,
`deu`…), and is answered in that language when the catalogue holds the series
in it; when it does not, in the language of the answers for that caller
(`tmdb.language`, on its key or on its address's rule); then in English; then
as the series is stored. A code that names no language is read as `eng`, as
Skyhook reads it, rather than refused. Every language the series is named in
travels with the answer too (`translations`), the series' own among them, which
is where a Sonarr v5 reads the original title from. Per-request language is
otherwise for the web UI, for anything calling this server directly, and for
TMDB clients — which do send `language=` and get it forwarded.

A translation never overwrites a field someone locked. That would undo a rename
the moment a client asked in another language, which is the one thing locking
exists to prevent. Lock a field per language if you want different text in each
([Locking a text for one language](editing.md#locking-a-text-for-one-language)).

What the relays write into the answers of TMDB clients, by the language they ask in, is in
[A note on languages](integration.md#a-note-on-languages).
