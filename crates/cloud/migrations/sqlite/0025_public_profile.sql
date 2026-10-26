-- The public profile (web /u/<handle>): 1 when the player shows a page to people who are not their
-- friends, signed in or not. Off by default: until the player turns it on, only the player and
-- their friends can open the page.
ALTER TABLE accounts ADD COLUMN public_profile INTEGER NOT NULL DEFAULT 0;
