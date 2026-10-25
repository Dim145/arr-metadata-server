# The command line

The binary is the server and its own administration tool: `arr-metadata-server` with no command
runs the server, and every command below reads the configuration the server reads — the same
variables, a `.env` beside it — and opens the same database. `--help` after any of them says
what it takes.

The image has no shell, and the binary is its entrypoint. Run a command in the container that
serves, or in a container of its own with the service's volume and environment:

```bash
# In the running container, by the binary's path:
docker exec -it arr-metadata-server /usr/local/bin/arr-metadata-server user list

# In a container of its own: Compose gives it the service's volume and variables, and what
# follows the service's name is the command.
docker compose run --rm metadata migrate status

# The same with Docker alone: the volume, the variables, the image, the command. Compose
# names a volume after its project — `docker volume ls` lists it.
docker run --rm -it -v arr-metadata-server_metadata-data:/data --env-file .env \
  ghcr.io/dim145/arr-metadata-server:latest config check
```

`-it` is for a command that asks for a password at the terminal. One read from standard input
— `--password-stdin`, `--passphrase-stdin` — needs `-i` alone, and nothing is ever taken as an
argument, which a process list and a shell's history would keep:

```bash
printf '%s\n' "$NEW_PASSWORD" | \
  docker exec -i arr-metadata-server /usr/local/bin/arr-metadata-server user reset-password alice --password-stdin
```

What changes accounts, keys or locks goes through the server's own API, in the process, as a key
named *command line* that exists nowhere else: the same checks as the interface — a username
taken, the last administrator, a scope that does not exist — the same audit trail, as
`client:command line`, the same webhooks. Such a command refuses a database whose migrations are
not exactly this binary's, rather than migrate it under a server that runs another version.

| Command | What it does |
| --- | --- |
| *(none)*, `serve` | Runs the server. |
| `healthcheck` | Asks the running server's `/health`, over loopback; exits non-zero if it is unwell. The image's health check. |
| `transfer FROM TO [--force]` | Copies a database into another, whatever their engines. |
| `backup [--out PATH] [--include-media] [--passphrase-stdin]` | Makes an archive. |
| `restore ARCHIVE [--replace] [--passphrase-stdin]` | Restores one. Refused while a server runs on the database. |
| `user list` | Lists the accounts. |
| `user create NAME [--role member\|editor\|admin]` | Opens an account. |
| `user reset-password NAME` | Gives an account a new password, and closes its sessions. |
| `user set-admin NAME [--revoke]` | Makes an account an active administrator, or a member again. |
| `keys create NAME --scope SCOPE…` | Issues a key of the server's own. |
| `locks export [--out FILE]`, `locks import FILE` | Every lock, as the document *Sources* exports and imports. |
| `config check` | Prints the configuration as the server reads it, and probes what it names. |
| `migrate status`, `migrate run` | The database's migrations, read and applied. |
| `version` | The version, the last migration and the archive layout it knows. |

## The server

`healthcheck` probes the address `AMS_BIND_ADDRESS` names: loopback for a server on every
interface, the bound address otherwise, in https when `AMS_TLS_CERT` is set.

`transfer` copies every row of the first database into the second, whose migrations are applied
first and which must be empty, unless `--force` adds to what it holds. The source is read as of
one moment, so a server may go on writing to it meanwhile; the copy is one transaction, so a
failure leaves the target as it was:

```bash
arr-metadata-server transfer 'sqlite://data/ams.db' 'postgres://ams:secret@localhost/ams'
```

## Backups

`backup` makes an archive where the server keeps them — `AMS_BACKUP_DIR`, or the bucket among
several instances — listed on **Administration › Backups** like any other, as made by the
*command line*. A server may be running: the copy is read as of one moment. `--include-media`
puts the media kept on disk in it; `--out PATH` writes it to a file instead, or into a directory
under its usual name, and does not list it; `--passphrase-stdin` encrypts it with a passphrase
read from standard input — asked for, unseen, at a terminal — rather than `AMS_BACKUP_PASSPHRASE`.
It says where it wrote the archive, its size and its SHA-256.

`restore` puts an archive back: a file, or the name of one the server keeps. It has the database
to itself, or it does not start: a server holding it — the server's own lock beside an SQLite
file, an advisory lock on PostgreSQL, wherever the server runs — or answering at the configured
address is a refusal, which says to stop it or to restore from the Backups page, where the server
shuts its own doors while it restores. A database that holds rows — and one a server started on
once holds its settings — is only restored over with `--replace`, which makes an archive of it
first: restoring that one undoes the restore. The archive's passphrase is `AMS_BACKUP_PASSPHRASE`,
or `--passphrase-stdin`'s.

```bash
docker compose stop metadata
docker compose run --rm metadata restore ams-20261019-020000-scheduled.tar.zst --replace
docker compose start metadata
```

What an archive holds, how it is encrypted and how it is opened without the server is in
[Installing and running](install.md#backups).

## Accounts and keys

`user create` opens an active account, a member unless `--role` says otherwise, with
`--display-name` and `--email` when given. Its password is read from standard input with
`--password-stdin`, asked for twice at a terminal, or made by the server with
`--generate-password` and shown once. The password is held to the rules the interface holds it to.

`user reset-password` gives an account a new password the same three ways and closes its
sessions. It is the way back in for an administrator who has lost theirs, beside
`AMS_ADMIN_USERNAME` and `AMS_ADMIN_PASSWORD`, which only act when no active administrator is
left.

`user set-admin` makes an account an active administrator — whatever its role or its status was —
and `--revoke` makes it a member; the last active administrator cannot be revoked.

`keys create` issues a key of the server's own, as **Keys & network** does: `--scope` once for
each of `read`, `write`, `admin`, `metrics` and `mcp` — an agent's, for the MCP server — `--expires`
with a date or an RFC 3339 instant, `--note`. The key is written to standard output, alone, and shown this once: only its hash is
kept.

```bash
docker exec arr-metadata-server /usr/local/bin/arr-metadata-server \
  keys create prometheus --scope metrics > prometheus.key
```

## Locks

`locks export` writes every lock — every manual edit — as the document **Sources** downloads, to
standard output or `--out FILE`. `locks import FILE` sets the locks of such a document on the
works held here, a part at a time as the interface sends it, and says how many were set, how many
were so already, which works are not held here and which locks were refused; `-` reads the
document from standard input. A work is found by the id it was exported with, else by its TMDB,
TheTVDB or IMDb id: the document travels from one catalogue to another.

## Configuration and migrations

`config check` prints the configuration as the server reads it — every secret masked, as the log
masks it — and then probes what it names: the database, and where its migrations stand; the media
store, a directory or a bucket; where the archives are kept, written to and listed; the public
address's `/health`. A configuration the server would refuse to start with is said as the start
would say it. It exits non-zero when the database, the store or the archives' place does not
answer; the public address is only reported, since a proxy may route it from outside alone.

`migrate status` lists this binary's migrations — applied, pending, or failed — and any the
database has that it does not know, which a later version applied. `migrate run` applies those
pending, with the database to itself as `restore` has it: a server applies its own as it starts,
and this is for an upgrade one wants to watch, or to try on a copy first. Either way the database
is archived first, as the version before left it, and the archive named — unless
`AMS_BACKUP_BEFORE_UPGRADE=false`, or `backup.beforeUpgrade` off on PostgreSQL
([Backups](install.md#backups)).

`version` prints the version, the last migration it knows on both engines, and the layout of the
archives it writes.
