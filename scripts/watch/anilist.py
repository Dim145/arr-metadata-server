"""AniList's schema, against its snapshot: what this server and its known
clients read there, what is deprecated, and what is gone.

AniList moves its schema without a word: `Media.averageScore` was deprecated
for `weightedScore` in the autumn of 2026, with no announcement and no change
to its documentation. So its schema is read here, by one introspection — a
single query, well within the thirty a minute AniList allows while it runs
degraded — and set beside snapshots/anilist.json.

Who reads what:
- this server's own queries, read where they are kept, in
  src/providers/anilist.rs: every string constant there that is a query;
- what the relay reads in the answers it passes on, and the known clients'
  queries: a file each under anilist-readers/, its first line its name.

Watched: every type one of them reads a field or a value of, whole — its
fields, their arguments, its values — and `Query` and `Page` too, since a new
field there is a new kind of answer, which the relay may have to treat as
somebody's. A field, an argument or a value that is new, gone, of another
type or newly deprecated is a move. AniList's introspection says whether a
field or a value is deprecated, not whether an argument is: an argument is
only there or gone.
"""

from __future__ import annotations

import json
import re
import time

from common import HERE, ROOT, Refused, Report, Unreadable, clean, fetch, keep, listed, shown, snapshot, today

ENDPOINT = "https://graphql.anilist.co"
SOURCES = [ROOT / "src" / "providers" / "anilist.rs"]
READERS = HERE / "anilist-readers"
THIS_SERVER = "this server"

# Fields and their arguments with their types, and the values of the enums,
# deprecated ones included. Arguments are asked without `includeDeprecated`,
# which AniList's server refuses.
INTROSPECTION = """
query {
  __schema {
    queryType { name }
    types {
      name
      kind
      fields(includeDeprecated: true) {
        name
        isDeprecated
        deprecationReason
        args { name description type { ...Ref } }
        type { ...Ref }
      }
      enumValues(includeDeprecated: true) { name isDeprecated deprecationReason }
    }
  }
}
fragment Ref on __Type {
  kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name } } } }
}
"""


def run(update):
    report = Report("anilist", "AniList's schema")
    before = snapshot("anilist")
    taken = before.get("taken") if before else None

    try:
        schema = Schema(read_schema())
    except (Unreadable, Refused, ValueError, KeyError, TypeError) as error:
        report.unreadable(f"AniList's schema: {clean(error, 300)}")
        return report.finish(update, taken)

    readers = []
    for name, documents, origin in reader_documents(report):
        read, absent = {}, set()
        for document in documents:
            try:
                operations, fragments = Parser(document).document()
            except ValueError as error:
                report.unreadable(f"{origin}: {clean(error)}")
                continue
            for root, selections in operations:
                resolve(selections, schema.roots.get(root, root), schema, fragments, read, absent)
        readers.append((name, read, absent))
    if report.unreadables:
        return report.finish(update, taken)

    read_by = {}
    for name, read, _ in readers:
        for coordinate in read:
            read_by.setdefault(coordinate, []).append(name)

    watched = {"Query", "Page"} | {coordinate.split(".")[0] for coordinate in read_by}
    state = schema.describe(watched)
    report.fact(
        f"{len(schema.types)} types read from `{ENDPOINT}`; {len(read_by)} fields, arguments and values "
        f"read by {listed(name for name, _, _ in readers)}; {len(state)} watched in {len(watched)} types."
    )

    def readers_of(coordinate):
        names = read_by.get(coordinate)
        return f" — read by {listed(names)}" if names else ""

    gone_or_changed = set()
    if before is None:
        report.move("there is no snapshot yet: take one.")
    else:
        kept = set(before.get("watched", []))
        common_types = kept & watched
        old = {c: v for c, v in before.get("schema", {}).items() if c.split(".")[0] in common_types}
        new = {c: v for c, v in state.items() if c.split(".")[0] in common_types}
        for name in sorted(watched - kept):
            report.move(f"`{name}` is read now and was not when the snapshot was taken: take it again.")
        for coordinate in sorted(new.keys() - old.keys()):
            hint = ""
            if coordinate.split(".")[0] in ("Query", "Page") and "(" not in coordinate:
                hint = (
                    " If it answers about somebody, the relay should never keep it "
                    "(`PERSONAL` in src/api/anilist.rs)."
                )
            report.move(f"`{coordinate}` is new: `{clean(new[coordinate])}`.{hint}")
        for coordinate in sorted(old.keys() - new.keys()):
            gone_or_changed.add(coordinate)
            report.move(f"`{coordinate}` is gone{readers_of(coordinate)}.")
        for coordinate in sorted(old.keys() & new.keys()):
            if old[coordinate] != new[coordinate]:
                gone_or_changed.add(coordinate)
                report.move(f"`{coordinate}` {changed(old[coordinate], new[coordinate])}{readers_of(coordinate)}.")

    for name, read, absent in readers:
        for coordinate in sorted(absent - gone_or_changed):
            report.move(f"`{coordinate}`, read by {name}, is not in AniList's schema.")

    for coordinate in sorted(read_by):
        reason = deprecation(state.get(coordinate, ""))
        if reason is not None and coordinate not in gone_or_changed:
            report.note(f"`{coordinate}` is deprecated: `{clean(reason)}`{readers_of(coordinate)}.")
        # An argument cannot be marked deprecated, but one that filters on a
        # deprecated field, `averageScore_greater` on `averageScore`, will go
        # with it.
        filtered = filters_on(coordinate, schema)
        if filtered and deprecation(schema.describe([filtered.split(".")[0]]).get(filtered, "")) is not None:
            report.note(f"`{coordinate}` filters on `{filtered}`, which is deprecated{readers_of(coordinate)}.")

    rows = [["Read", "By", "In AniList's schema"]]
    for coordinate in sorted(read_by):
        rows.append([f"`{coordinate}`", listed(read_by[coordinate], 8), clean(state.get(coordinate, "not there"))])
    report.detail(f"The {len(read_by)} fields, arguments and values read", rows)

    if update and not report.unreadables:
        path = keep("anilist", {"taken": today(), "endpoint": ENDPOINT, "watched": sorted(watched), "schema": state})
        report.fact(f"Snapshot written to {shown(path)}.")
    return report.finish(update, taken)


def filters_on(coordinate, schema):
    """The field an argument filters on, `Media.averageScore` for
    `Page.media(averageScore_greater:)`, when the field it is given to
    returns entries that have one of its name."""
    match = re.fullmatch(r"(\w+)\.(\w+)\((\w+):\)", coordinate)
    if not match:
        return None
    owner, field, argument = match.groups()
    returned = schema.fields.get(owner, {}).get(field)
    if returned is None:
        return None
    entry = named(returned["type"])
    base = re.sub(r"_(not_in|not|in|greater|lesser|like)$", "", argument)
    return f"{entry}.{base}" if base in schema.fields.get(entry, {}) else None


def read_schema(tries=3):
    """The schema, by introspection. Besides what `fetch` tries again, an
    answer that is not a schema — a page of its edge's instead of JSON, a
    403 while AniList switches its API off for a while, as it says it may —
    is asked again twice, twenty seconds apart."""
    body = json.dumps({"query": INTROSPECTION}).encode()
    headers = {"Content-Type": "application/json", "Accept": "application/json"}
    for attempt in range(1, tries + 1):
        try:
            answer = json.loads(fetch(ENDPOINT, data=body, headers=headers, method="POST"))
            schema = (answer.get("data") or {}).get("__schema") if isinstance(answer, dict) else None
            if not schema:
                errors = answer.get("errors") if isinstance(answer, dict) else answer
                raise ValueError(f"AniList answered no schema: {clean(json.dumps(errors), 200)}")
            return schema
        except (ValueError, Refused) as error:
            if attempt == tries or (isinstance(error, Refused) and error.status != 403):
                raise
        time.sleep(20)
    raise AssertionError("every try returns or raises")


def reader_documents(report):
    """Each reader's name, its GraphQL documents, and where they were read."""
    for source in SOURCES:
        text = source.read_text(encoding="utf-8")
        lists = rust_lists(text)
        queries = [fill(s, lists) for s in rust_strings(text) if re.match(r"\s*(query\b|fragment\b|\{)", s)]
        if not queries:
            report.unreadable(
                f"no query found in {shown(source)}, where this watch reads this server's own: "
                "a string constant beginning with `query`."
            )
        else:
            report.fact(f"{len(queries)} queries of this server's, read in {shown(source)}.")
        yield THIS_SERVER, queries, shown(source)
    for path in sorted(READERS.glob("*.graphql")):
        text = path.read_text(encoding="utf-8")
        first = text.splitlines()[0] if text.strip() else ""
        name = first[1:].split(" — ")[0].strip() if first.startswith("#") else ""
        yield name or path.stem, [text], shown(path)


# ─── Rust ───────────────────────────────────────────────────────────────────

RUST_CONST = re.compile(
    r"const\s+[A-Z][A-Z0-9_]*\s*:\s*&(?:'static\s+)?str\s*=\s*"
    r'(?:r(?P<hashes>#*)"(?P<raw>.*?)"(?P=hashes)|"(?P<text>(?:[^"\\]|\\.)*)")',
    re.S,
)
RUST_LIST = re.compile(
    r"const\s+(?P<name>[A-Z][A-Z0-9_]*)\s*:\s*"
    r"(?:\[&(?:'static\s+)?str\s*;\s*\d+\]|&(?:'static\s+)?\[&(?:'static\s+)?str\])"
    r"\s*=\s*&?\[(?P<items>[^\]]*)\]\s*;",
    re.S,
)
RUST_ITEM = re.compile(r'"((?:[^"\\]|\\.)*)"')
RUST_ESCAPES = {"n": "\n", "r": "\r", "t": "\t", "0": "\0", "\\": "\\", '"': '"', "'": "'"}


def rust_strings(source):
    """The values of the `&str` constants of a Rust file."""
    for match in RUST_CONST.finditer(source):
        if match.group("raw") is not None:
            yield match.group("raw")
        else:
            yield unescape(match.group("text"))


def rust_lists(source):
    """The `&str` arrays and slices of a Rust file, by name."""
    return {m.group("name"): [unescape(i) for i in RUST_ITEM.findall(m.group("items"))] for m in RUST_LIST.finditer(source)}


def fill(query, lists):
    """A query with its placeholders filled as the code fills them before
    sending it: `{scores}` becomes the names `SCORES` holds, all of them,
    since the code leaves one out only once AniList refuses it. A
    placeholder with no list of its name is left, and then reads as a field
    the schema does not have — which shows."""

    def one(match):
        names = lists.get(match.group(1).upper())
        return " ".join(names) if names else match.group(0)

    return re.sub(r"\{([a-z][a-z0-9_]*)\}", one, query)


def unescape(text):
    # A backslash at the end of a line joins it to the next, without the
    # next one's indentation.
    text = re.sub(r"\\\n\s*", "", text)

    def one(match):
        escape = match.group(1)
        if escape.startswith("u{"):
            return chr(int(escape[2:-1], 16))
        if escape.startswith("x"):
            return chr(int(escape[1:], 16))
        return RUST_ESCAPES.get(escape, escape)

    return re.sub(r"\\(u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)", one, text)


# ─── GraphQL ────────────────────────────────────────────────────────────────

TOKEN = re.compile(
    r"""
      (?P<skip>[\s,\ufeff]+|\#[^\n\r]*)
    | (?P<block>\"\"\"(?:\\\"\"\"|[^"]|"(?!""))*\"\"\")
    | (?P<string>"(?:\\.|[^"\\\n])*")
    | (?P<spread>\.\.\.)
    | (?P<punct>[!$&():=@\[\]{|}])
    | (?P<number>-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)
    | (?P<name>[_A-Za-z][_0-9A-Za-z]*)
    """,
    re.X,
)


class Parser:
    """Just enough of GraphQL to tell what a query selects: its fields, the
    arguments given them, the enum values written in those arguments, and
    the fragments and inline fragments that select more. It does not check
    a query against the schema: `resolve` does."""

    def __init__(self, text):
        self.tokens, position = [], 0
        while position < len(text):
            match = TOKEN.match(text, position)
            if not match:
                line = text.count("\n", 0, position) + 1
                raise ValueError(f"cannot read the query at line {line}: {text[position:position + 20]!r}")
            if match.lastgroup != "skip":
                self.tokens.append((match.lastgroup, match.group()))
            position = match.end()
        self.position = 0

    def peek(self):
        return self.tokens[self.position] if self.position < len(self.tokens) else (None, None)

    def at(self, kind, value=None):
        token = self.peek()
        return token[0] == kind and (value is None or token[1] == value)

    def take(self, kind=None, value=None):
        token = self.peek()
        if token[0] is None or (kind and token[0] != kind) or (value and token[1] != value):
            raise ValueError(f"expected {value or kind}, found {token[1]!r}")
        self.position += 1
        return token[1]

    def document(self):
        """The operations — each its kind and its selections — and the fragments, by name."""
        operations, fragments = [], {}
        while self.peek()[0] is not None:
            if self.at("punct", "{"):
                operations.append(("query", self.selections()))
            elif self.at("name", "fragment"):
                self.take()
                name = self.take("name")
                self.take("name", "on")
                on = self.take("name")
                self.directives()
                fragments[name] = (on, self.selections())
            else:
                kind = self.take("name")
                if kind not in ("query", "mutation", "subscription"):
                    raise ValueError(f"expected an operation, found {kind!r}")
                if self.at("name"):
                    self.take()
                if self.at("punct", "("):
                    self.skip("(", ")")
                self.directives()
                operations.append((kind, self.selections()))
        return operations, fragments

    def selections(self):
        self.take("punct", "{")
        found = []
        while not self.at("punct", "}"):
            if self.at("spread"):
                self.take()
                if self.at("name", "on"):
                    self.take()
                    on = self.take("name")
                    self.directives()
                    found.append(("inline", on, self.selections()))
                elif self.at("name"):
                    name = self.take()
                    self.directives()
                    found.append(("spread", name))
                else:
                    self.directives()
                    found.append(("inline", None, self.selections()))
                continue
            name = self.take("name")
            if self.at("punct", ":"):
                # An alias: the field is the name after it.
                self.take()
                name = self.take("name")
            arguments = self.arguments() if self.at("punct", "(") else {}
            self.directives()
            below = self.selections() if self.at("punct", "{") else None
            found.append(("field", name, arguments, below))
        self.take("punct", "}")
        return found

    def arguments(self):
        self.take("punct", "(")
        found = {}
        while not self.at("punct", ")"):
            name = self.take("name")
            self.take("punct", ":")
            found[name] = self.value()
        self.take("punct", ")")
        return found

    def value(self):
        """The enum values a value names, and nothing else of it."""
        kind, text = self.peek()
        if (kind, text) == ("punct", "$"):
            self.take()
            self.take("name")
            return []
        if (kind, text) == ("punct", "["):
            self.take()
            names = []
            while not self.at("punct", "]"):
                names += self.value()
            self.take()
            return names
        if (kind, text) == ("punct", "{"):
            self.take()
            while not self.at("punct", "}"):
                self.take("name")
                self.take("punct", ":")
                self.value()
            self.take()
            return []
        if kind == "name":
            self.take()
            return [] if text in ("true", "false", "null") else [text]
        if kind in ("number", "string", "block"):
            self.take()
            return []
        raise ValueError(f"expected a value, found {text!r}")

    def directives(self):
        while self.at("punct", "@"):
            self.take()
            self.take("name")
            if self.at("punct", "("):
                self.arguments()

    def skip(self, opening, closing):
        depth = 0
        while True:
            text = self.take()
            depth += {opening: 1, closing: -1}.get(text, 0)
            if depth == 0:
                return


def resolve(selections, type_name, schema, fragments, read, absent):
    """Every coordinate a selection reads, from the type it is made on:
    `Type.field`, `Type.field(argument:)`, `Enum.VALUE`. What the schema does
    not have goes in `absent` as well."""
    for selection in selections:
        if selection[0] == "inline":
            resolve(selection[2], selection[1] or type_name, schema, fragments, read, absent)
            continue
        if selection[0] == "spread":
            if selection[1] in fragments:
                on, below = fragments[selection[1]]
                resolve(below, on, schema, fragments, read, absent)
            continue
        _, name, arguments, below = selection
        if name.startswith("__"):
            continue
        coordinate = f"{type_name}.{name}"
        read[coordinate] = True
        field = schema.fields.get(type_name, {}).get(name)
        if field is None:
            absent.add(coordinate)
            continue
        given = {argument["name"]: argument for argument in field["args"]}
        for argument, values in arguments.items():
            read[f"{coordinate}({argument}:)"] = True
            if argument not in given:
                absent.add(f"{coordinate}({argument}:)")
                continue
            enum = named(given[argument]["type"])
            if schema.kinds.get(enum) == "ENUM":
                for value in values:
                    read[f"{enum}.{value}"] = True
                    if value not in schema.values.get(enum, {}):
                        absent.add(f"{enum}.{value}")
        if below:
            resolve(below, named(field["type"]), schema, fragments, read, absent)


# ─── the schema ─────────────────────────────────────────────────────────────


class Schema:
    def __init__(self, raw):
        self.types = {t["name"]: t for t in raw["types"]}
        self.kinds = {name: t["kind"] for name, t in self.types.items()}
        self.fields = {name: {f["name"]: f for f in t.get("fields") or []} for name, t in self.types.items()}
        self.values = {name: {v["name"]: v for v in t.get("enumValues") or []} for name, t in self.types.items()}
        self.roots = {"query": raw["queryType"]["name"]}

    def describe(self, watched):
        """Each coordinate of the watched types, and what it is: a field's
        type and its deprecation, an argument's type — with its description
        when it is a `version`, which is where AniList says what each
        version of an enum holds — and a value's deprecation."""
        state = {}
        for name in sorted(watched):
            for field in self.fields.get(name, {}).values():
                coordinate = f"{name}.{field['name']}"
                state[coordinate] = signature(field["type"]) + deprecated(field)
                for argument in field["args"]:
                    text = signature(argument["type"])
                    if argument["name"] == "version" and argument.get("description"):
                        text += f"; {' '.join(argument['description'].split())}"
                    state[f"{coordinate}({argument['name']}:)"] = text
            for value in self.values.get(name, {}).values():
                state[f"{name}.{value['name']}"] = "enum value" + deprecated(value)
        return state


def named(reference):
    while reference.get("ofType") and reference["kind"] in ("NON_NULL", "LIST"):
        reference = reference["ofType"]
    return reference["name"]


def signature(reference):
    if reference["kind"] == "NON_NULL":
        return signature(reference["ofType"]) + "!"
    if reference["kind"] == "LIST":
        return f"[{signature(reference['ofType'])}]"
    return reference["name"]


def deprecated(item):
    if not item.get("isDeprecated"):
        return ""
    return f"; deprecated: {' '.join((item.get('deprecationReason') or 'no reason given').split())}"


def deprecation(text):
    """The reason of a deprecation, in what `describe` says; None when there is none."""
    _, marker, reason = text.partition("; deprecated: ")
    return reason if marker else None


def changed(old, new):
    """What became of a coordinate, in words."""
    was, now = deprecation(old), deprecation(new)
    if was is None and now is not None:
        return f"is now deprecated: `{clean(now)}`"
    if was is not None and now is None:
        return "is no longer deprecated"
    if was != now:
        return f"is deprecated for another reason: `{clean(now)}` (was `{clean(was)}`)"
    return f"is now `{clean(new)}` (was `{clean(old)}`)"
