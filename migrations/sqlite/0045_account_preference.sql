-- What a person keeps of their own way of working, with their account, so
-- that it follows them from one browser to the next: the views of the
-- catalogue they saved, which columns a table shows them and how tightly it
-- is set. A row a preference, its value the JSON the interface wrote, and
-- gone with the account.
CREATE TABLE account_preference (
    user_id    TEXT NOT NULL REFERENCES admin_user (id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, key)
);
