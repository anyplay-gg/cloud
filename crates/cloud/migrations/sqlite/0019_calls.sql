-- Calls and the party's Discord link (network ADR 0026 and spec/cloud-api.md, "Calls" and "Parties").

-- A call is a party invitation that rings: 1 for a call.
ALTER TABLE party_invitations ADD COLUMN call INTEGER NOT NULL DEFAULT 0;

-- Where the party talks when its voice is on Discord; NULL when the leader named none.
ALTER TABLE parties ADD COLUMN discord TEXT;

-- A call nobody answered, shown to the callee until seen. One row per caller: a newer call
-- replaces the older. `at` is Unix seconds.
CREATE TABLE missed_calls (
    account_id  INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    from_id     INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    from_handle TEXT    NOT NULL,
    at          INTEGER NOT NULL,
    PRIMARY KEY (account_id, from_id)
) STRICT;

CREATE INDEX missed_calls_at ON missed_calls (at);
