-- The offline sweep (`live::sweep_offline`) runs every few seconds on every node and looks for
-- the few accounts that are online with no connection. `presence` holds one row per account ever
-- seen, so without this index each sweep reads all of them. The index holds only the accounts
-- that can be swept, which is a small, short-lived set.
CREATE INDEX presence_due ON presence (last_seen_ms) WHERE online = 1 AND last_seen_ms IS NOT NULL;
