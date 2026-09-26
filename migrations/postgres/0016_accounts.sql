-- Every account, not only administrators.
--
-- The table keeps its name — the sessions, the transfer and older rows all
-- name it — and gains what a person needs: a role, a status, what they are
-- called, and the identity provider that vouches for them when one does.
-- Roles: `admin` settles everything, `editor` corrects the catalogue,
-- `member` browses and holds keys of their own. Statuses: `active`,
-- `pending` (signed up, waiting for an administrator), `disabled`.
ALTER TABLE admin_user ADD COLUMN role TEXT NOT NULL DEFAULT 'member';
UPDATE admin_user SET role = CASE WHEN is_admin = 1 THEN 'admin' ELSE 'editor' END;
ALTER TABLE admin_user ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE admin_user ADD COLUMN display_name TEXT;
ALTER TABLE admin_user ADD COLUMN email TEXT;
-- The interface's language, when the person chose one.
ALTER TABLE admin_user ADD COLUMN locale TEXT;
-- The OpenID Connect issuer and subject that name this account there.
ALTER TABLE admin_user ADD COLUMN oidc_issuer TEXT;
ALTER TABLE admin_user ADD COLUMN oidc_subject TEXT;
ALTER TABLE admin_user ADD COLUMN invited_by TEXT;
ALTER TABLE admin_user ADD COLUMN updated_at TEXT;

CREATE UNIQUE INDEX ux_admin_user_oidc ON admin_user (oidc_issuer, oidc_subject);
CREATE INDEX ix_admin_user_role ON admin_user (role, status);
-- A username is looked up without regard to case, so it is unique that way
-- too: "Leo" and "leo" would be one person to the sign-in page and two to the
-- table.
CREATE UNIQUE INDEX ux_admin_user_username_ci ON admin_user (LOWER(username));

-- When a session was last used, for the list of a person's devices.
ALTER TABLE admin_session ADD COLUMN last_seen_at TEXT;

-- A key may belong to a person, and acts with no more than their role
-- allows. Its name is unique per owner now — the server's own keys counting
-- as one owner — so two people may each call theirs "Sonarr". The code also
-- compares names without regard to case; the index holds the line when two
-- requests race past that check.
DROP INDEX ux_api_client_name;
ALTER TABLE api_client ADD COLUMN owner_id TEXT REFERENCES admin_user (id) ON DELETE CASCADE;
CREATE INDEX ix_api_client_owner ON api_client (owner_id);
CREATE UNIQUE INDEX ux_api_client_owner_name ON api_client ((COALESCE(owner_id, '')), name);
