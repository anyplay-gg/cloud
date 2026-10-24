-- What the web portal's account API needs beyond the account web: a deletion with a grace
-- period, and the time a browser session was last used (network spec/portal-api.md).

-- When the account will be deleted, in Unix seconds, after the player asked to delete it. Null
-- while no deletion is scheduled. The sweep deletes the account once the time has passed; the
-- player can cancel until then.
ALTER TABLE accounts ADD COLUMN deletion_at INTEGER;

CREATE INDEX accounts_deletion ON accounts (deletion_at) WHERE deletion_at IS NOT NULL;

-- When the browser session was last used, in milliseconds. Null for sessions made before this.
ALTER TABLE web_sessions ADD COLUMN used_ms INTEGER;
