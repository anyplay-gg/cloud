-- Moderation: moderators, reports with a status and a snapshot, actions with a reason the player
-- reads, appeals, and an append-only audit log (docs/moderation.md). Number 0004 belongs to
-- another change.
--
-- Accounts, reports and actions are tied together by plain numbers, not foreign keys: a report
-- and an action must outlive the accounts they name until their retention ends.

-- Null for a player; the Unix second the operator granted the moderator role otherwise.
ALTER TABLE accounts ADD COLUMN moderator_since INTEGER;

-- Reports no longer vanish with an account. The reporter is forgotten when they delete their
-- account (the cloud nulls `reporter` and blanks the handle); the reported side stays as the
-- evidence. `content` is a snapshot of what was reported.
CREATE TABLE reports_new (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    reporter        INTEGER,
    reporter_handle TEXT    NOT NULL,
    reported        INTEGER NOT NULL,
    reported_handle TEXT    NOT NULL,
    reason          TEXT    NOT NULL,
    details         TEXT    NOT NULL,
    content         TEXT    NOT NULL,
    created         INTEGER NOT NULL,
    -- open | decided
    status          TEXT    NOT NULL,
    -- dismissed | actioned, once decided
    outcome         TEXT,
    decided         INTEGER,
    decided_by      INTEGER,
    decided_by_handle TEXT,
    action_id       INTEGER
) STRICT;

INSERT INTO reports_new (id, reporter, reporter_handle, reported, reported_handle, reason,
                         details, content, created, status)
SELECT r.id, r.reporter, ar.handle, r.reported, ad.handle, r.reason, r.details, '', r.created, 'open'
FROM reports r
JOIN accounts ar ON ar.id = r.reporter
JOIN accounts ad ON ad.id = r.reported;

DROP TABLE reports;
ALTER TABLE reports_new RENAME TO reports;

CREATE INDEX reports_reporter ON reports (reporter, created);
CREATE INDEX reports_reported ON reports (reported);
CREATE INDEX reports_status ON reports (status, created);

-- What a moderator did to an account, and the statement of reasons the player reads.
CREATE TABLE actions (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id     INTEGER NOT NULL,
    account_handle TEXT    NOT NULL,
    -- warning | mute | ban
    kind           TEXT    NOT NULL,
    rule           TEXT    NOT NULL,
    reason         TEXT    NOT NULL,
    created        INTEGER NOT NULL,
    -- The end of a mute or ban; null for a warning and for a ban for good.
    until          INTEGER,
    -- The moderator's account ID; null for the operator's admin token.
    actor          INTEGER,
    actor_handle   TEXT    NOT NULL,
    report_id      INTEGER,
    -- When the action was taken back.
    lifted         INTEGER
) STRICT;

CREATE INDEX actions_account ON actions (account_id, created);

-- At most one appeal per action.
CREATE TABLE appeals (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    action_id         INTEGER NOT NULL UNIQUE REFERENCES actions (id) ON DELETE CASCADE,
    account_id        INTEGER NOT NULL,
    text              TEXT    NOT NULL,
    created           INTEGER NOT NULL,
    -- open | upheld | reversed
    status            TEXT    NOT NULL,
    decided           INTEGER,
    decided_by        INTEGER,
    decided_by_handle TEXT,
    answer            TEXT
) STRICT;

CREATE INDEX appeals_status ON appeals (status, created);

-- Only ever added to. The triggers refuse a change; the retention sweep deletes old lines.
CREATE TABLE audit_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    created       INTEGER NOT NULL,
    actor         INTEGER,
    actor_handle  TEXT    NOT NULL,
    action        TEXT    NOT NULL,
    target        INTEGER,
    target_handle TEXT,
    reason        TEXT    NOT NULL,
    rule          TEXT,
    -- report | action | appeal
    ref_kind      TEXT,
    ref_id        INTEGER
) STRICT;

CREATE INDEX audit_log_created ON audit_log (created);

CREATE TRIGGER audit_log_no_update BEFORE UPDATE ON audit_log
BEGIN
    SELECT RAISE(ABORT, 'the audit log is append-only');
END;

-- A ban points at its statement. `salted` is 1 once `email_hash` is a keyed hash of the address
-- (HMAC-SHA-256 with the cloud's ban key) and 0 for a hash made before this migration.
ALTER TABLE bans ADD COLUMN action_id INTEGER;
ALTER TABLE bans ADD COLUMN salted INTEGER NOT NULL DEFAULT 0;

-- Bans that exist get a statement too, so their owners can read the reason.
INSERT INTO actions (account_id, account_handle, kind, rule, reason, created, until, actor, actor_handle)
SELECT b.account_id, COALESCE(a.handle, ''), 'ban', 'other', b.reason, b.created, b.until, NULL, 'operator'
FROM bans b
LEFT JOIN accounts a ON a.id = b.account_id;

UPDATE bans SET action_id =
    (SELECT MAX(x.id) FROM actions x WHERE x.account_id = bans.account_id AND x.kind = 'ban');

INSERT INTO audit_log (created, actor, actor_handle, action, target, target_handle, reason, rule, ref_kind, ref_id)
SELECT x.created, NULL, 'operator', 'banned', x.account_id, x.account_handle, x.reason, x.rule, 'action', x.id
FROM actions x;
