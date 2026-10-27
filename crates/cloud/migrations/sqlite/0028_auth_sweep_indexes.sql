-- The sweep of expired sign-in challenges, sign-ups and invitations deletes by deadline, and
-- issuing a new code removes the older codes of the same address. Without these indexes each of
-- them reads the whole table, which a flood of requests can make large.
CREATE INDEX challenges_expires ON challenges (expires_ms);
CREATE INDEX challenges_email ON challenges (email);
CREATE INDEX signups_expires ON signups (expires_ms);
CREATE INDEX invitations_deadline ON invitations (deadline_ms);
