-- Web Push subscriptions, set in the portal as Notifications (network spec/portal-api.md,
-- "Notifications (Web Push)"): the address of a browser's push service and the keys that encrypt
-- what the cloud sends to it. One row per browser; an address belongs to one account at a time.
CREATE TABLE push_subscriptions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- The address of the push service for this browser. Unique: a browser signed in as another
    -- account moves its row.
    endpoint   TEXT    NOT NULL UNIQUE,
    -- The browser's public key and authentication secret, unpadded base64url.
    p256dh     TEXT    NOT NULL,
    auth       TEXT    NOT NULL,
    -- A name the player gave the device. Empty when none.
    label      TEXT    NOT NULL DEFAULT '',
    -- The kinds to send, comma separated: friend_request, party_invite, message.
    kinds      TEXT    NOT NULL,
    -- Failed sends in a row; the row goes at 5, or at once when the push service says it is gone.
    failures   INTEGER NOT NULL DEFAULT 0,
    -- Unix seconds.
    created_at INTEGER NOT NULL,
    last_ok_at INTEGER
);

CREATE INDEX push_subscriptions_account ON push_subscriptions (account_id);
