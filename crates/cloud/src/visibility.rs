//! Who may see which part of an account: the one rule for trophies, the text, favourites, avatar
//! and playtime.
//!
//! The account itself always; its friends unless it shows the part to nobody; everyone else only
//! if it shows the part to everyone. A block in either direction hides every part.

use anyplay_api::{Audience, Part, Privacy};

use crate::db::{self, Pool};

/// How the viewer stands to the owner of what they look at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    Own,
    Blocked,
    Friend,
    Stranger,
}

/// What a viewer may see of one owner. Reading it costs a few queries; ask once per owner and
/// then ask for each part.
#[derive(Clone, Copy, Debug)]
pub struct Viewing {
    relation: Relation,
    privacy: Privacy,
}

impl Viewing {
    pub fn new(relation: Relation, privacy: Privacy) -> Self {
        Self { relation, privacy }
    }

    pub fn relation(&self) -> Relation {
        self.relation
    }

    /// True when the viewer may see `part`.
    pub fn can(&self, part: Part) -> bool {
        match self.relation {
            Relation::Own => true,
            Relation::Blocked => false,
            Relation::Friend => self.privacy.audience(part) != Audience::Nobody,
            Relation::Stranger => self.privacy.audience(part) == Audience::Everyone,
        }
    }
}

/// How `viewer` stands to `owner` and what `owner` shows.
pub async fn viewing(pool: &Pool, viewer: u64, owner: u64) -> sqlx::Result<Viewing> {
    if viewer == owner {
        return Ok(Viewing::new(Relation::Own, Privacy::default()));
    }
    if db::blocked_either(pool, viewer, owner).await? {
        return Ok(Viewing::new(Relation::Blocked, Privacy::default()));
    }
    let relation = if db::are_friends(pool, viewer, owner).await? {
        Relation::Friend
    } else {
        Relation::Stranger
    };
    Ok(Viewing::new(relation, db::privacy_of(pool, owner).await?))
}

/// What a viewer sees on a public page, as the web portal shows it. The viewer may be signed
/// out, and then is a stranger. A viewer who is blocked either way is a stranger too: a page that
/// showed a blocked viewer less than a visitor who is not signed in would tell them about the
/// block. (A block still keeps them from friend-only parts, as any stranger is.)
pub async fn viewing_as(pool: &Pool, viewer: Option<u64>, owner: u64) -> sqlx::Result<Viewing> {
    let stranger = || async {
        Ok(Viewing::new(
            Relation::Stranger,
            db::privacy_of(pool, owner).await?,
        ))
    };
    match viewer {
        None => stranger().await,
        Some(viewer) => match viewing(pool, viewer, owner).await? {
            seen if seen.relation() == Relation::Blocked => stranger().await,
            seen => Ok(seen),
        },
    }
}

/// True when `viewer` may see `part` of `owner`.
pub async fn may_see(pool: &Pool, viewer: u64, owner: u64, part: Part) -> sqlx::Result<bool> {
    Ok(viewing(pool, viewer, owner).await?.can(part))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_relation_follows_the_audience() {
        let shows = |audience| Privacy {
            avatar: audience,
            ..Privacy::default()
        };
        for (relation, friends, everyone, nobody) in [
            (Relation::Own, true, true, true),
            (Relation::Friend, true, true, false),
            (Relation::Stranger, false, true, false),
            (Relation::Blocked, false, false, false),
        ] {
            for (audience, expected) in [
                (Audience::Friends, friends),
                (Audience::Everyone, everyone),
                (Audience::Nobody, nobody),
            ] {
                let viewing = Viewing::new(relation, shows(audience));
                assert_eq!(
                    viewing.can(Part::Avatar),
                    expected,
                    "{relation:?} {audience:?}"
                );
                assert_eq!(
                    viewing.can(Part::About),
                    relation == Relation::Own || relation == Relation::Friend,
                    "{relation:?} {audience:?}: the other parts keep friends"
                );
            }
        }
    }
}
