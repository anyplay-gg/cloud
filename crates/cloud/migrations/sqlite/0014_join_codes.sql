-- Join codes (network spec/cloud-api.md, "Join codes"): a short random token that stands for an
-- account and lets someone who is not a friend ask to join the game it offers. The table holds a
-- hash of the code and never the code; an account has at most one, and issuing a new one replaces
-- the old. `expires` is Unix seconds. A sweep deletes the codes that ran out.
CREATE TABLE join_codes (
    account_id INTEGER PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    code_hash  BLOB    NOT NULL UNIQUE,
    expires    INTEGER NOT NULL,
    created    INTEGER NOT NULL
) STRICT;

CREATE INDEX join_codes_expires ON join_codes (expires);
