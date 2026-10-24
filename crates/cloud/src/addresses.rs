//! Virtual addresses of session members (`network/docs/adr/0020-virtual-addresses-belong-to-sessions.md`).
//!
//! An address belongs to a member of a session and differs from every other member's in that
//! session. Nothing else is shared between sessions, so the store keeps no table of addresses:
//! the member rows are the record, and a session picks from what its own members hold.

use std::collections::HashSet;
use std::net::Ipv4Addr;

use anyplay_api::{VIRTUAL_ADDRESSES, is_assignable, virtual_address};

use crate::util::sha256;

/// The address an account is given in a session whenever no member of it holds that address:
/// the same every time. It is where the account's search for a free address starts.
pub fn preferred(account: u64) -> Ipv4Addr {
    let hash = sha256(&account.to_be_bytes());
    let n = u64::from_be_bytes([
        hash[0], hash[1], hash[2], hash[3], hash[4], hash[5], hash[6], hash[7],
    ]);
    first_free_from((n % u64::from(VIRTUAL_ADDRESSES)) as u32, &HashSet::new())
}

/// The address for an account joining a session whose members hold `held`: its preferred
/// address, or the next free one after it. A session holds far fewer members than the range has
/// addresses ([`anyplay_api::ASSIGNABLE_ADDRESSES`]), so there always is one.
pub fn for_new_member(account: u64, held: impl IntoIterator<Item = Ipv4Addr>) -> Ipv4Addr {
    let held: HashSet<Ipv4Addr> = held.into_iter().collect();
    let start = anyplay_api::virtual_slot(preferred(account)).unwrap_or(0);
    first_free_from(start, &held)
}

fn first_free_from(start: u32, held: &HashSet<Ipv4Addr>) -> Ipv4Addr {
    (0..VIRTUAL_ADDRESSES)
        .filter_map(|step| virtual_address((start + step) % VIRTUAL_ADDRESSES))
        .find(|address| is_assignable(*address) && !held.contains(address))
        .expect("a session cannot hold every address of the range")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preferred_address_is_assignable_and_always_the_same() {
        for account in 1..=2_000 {
            let address = preferred(account);
            assert!(is_assignable(address), "{address}");
            assert_eq!(address, preferred(account));
        }
    }

    #[test]
    fn a_member_gets_its_preferred_address_when_no_one_holds_it() {
        assert_eq!(for_new_member(7, []), preferred(7));
        assert_eq!(for_new_member(7, [preferred(8)]), preferred(7));
    }

    #[test]
    fn the_same_account_gets_the_same_address_in_sessions_that_do_not_meet() {
        // Nothing is shared between sessions: any number of them give one address to one player.
        for _ in 0..3 {
            assert_eq!(
                for_new_member(42, [preferred(1), preferred(2)]),
                preferred(42)
            );
        }
    }

    #[test]
    fn a_taken_address_moves_the_newcomer_to_the_next_free_one() {
        let wanted = preferred(7);
        let next = for_new_member(7, [wanted]);
        assert_ne!(next, wanted);
        assert!(is_assignable(next));
        // The next free one: held addresses in a row are skipped one by one.
        let mut held = vec![wanted];
        for _ in 0..5 {
            let address = for_new_member(7, held.iter().copied());
            assert!(!held.contains(&address));
            held.push(address);
        }
    }

    #[test]
    fn a_full_session_of_accounts_with_one_preferred_address_gets_distinct_addresses() {
        // Worst case: everyone wants the same address, as if all hashes collided.
        let mut held: Vec<Ipv4Addr> = Vec::new();
        for _ in 0..u32::from(anyplay_api::MAX_MEMBERS) {
            let address = for_new_member(1, held.iter().copied());
            assert!(is_assignable(address));
            assert!(!held.contains(&address));
            held.push(address);
        }
    }

    #[test]
    fn the_search_wraps_at_the_end_of_the_range_and_skips_block_ends() {
        let last = Ipv4Addr::new(198, 19, 255, 254);
        let held: HashSet<Ipv4Addr> = [last].into();
        let start = anyplay_api::virtual_slot(last).unwrap();
        // .255 of the last block and .0 of the first are skipped, so the next one is .1.
        assert_eq!(first_free_from(start, &held), Ipv4Addr::new(198, 18, 0, 1));
        assert_eq!(
            first_free_from(start - 1, &held),
            Ipv4Addr::new(198, 19, 255, 253)
        );
    }

    #[test]
    fn preferred_addresses_spread_over_the_range() {
        let spread: HashSet<Ipv4Addr> = (1..=1_000).map(preferred).collect();
        // 1,000 accounts into 130,048 addresses: a few collisions at most.
        assert!(spread.len() > 980, "{}", spread.len());
    }
}
