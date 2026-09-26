-- Invitations: a code an administrator hands out, which opens the sign-up
-- page and gives the account it makes a role. Only the code's SHA-256 is
-- kept, as for keys: the code itself is shown once, in the link.
CREATE TABLE user_invitation (
    id           TEXT    PRIMARY KEY,
    code_hash    TEXT    NOT NULL,
    -- The code's visible head, `inv_4Q2F`, to tell invitations apart.
    code_prefix  TEXT    NOT NULL,
    role         TEXT    NOT NULL DEFAULT 'member',
    max_uses     INTEGER NOT NULL DEFAULT 1,
    uses         INTEGER NOT NULL DEFAULT 0,
    expires_at   TEXT,
    note         TEXT,
    created_by   TEXT    REFERENCES admin_user (id) ON DELETE SET NULL,
    created_at   TEXT    NOT NULL,
    revoked_at   TEXT
);

CREATE UNIQUE INDEX ux_user_invitation_code ON user_invitation (code_hash);
