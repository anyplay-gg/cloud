-- Play Together (network ADR 0030): what a session is for. 'game' is a session the game opened
-- through its own calls and what every earlier row is; 'lan' is a Play Together session that
-- ends with its leader. An invitation carries the kind of its session, so a client can word it.
ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'game';
ALTER TABLE invitations ADD COLUMN kind TEXT NOT NULL DEFAULT 'game';
