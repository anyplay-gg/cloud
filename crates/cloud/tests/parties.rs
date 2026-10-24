mod common;

use std::time::Duration;

use anyplay_api::{
    AccountId, Conversation, Event, Invitation, Invited, MAX_PARTY_MEMBERS, Message, Messages,
    Party, PartyInvitation, PartyInvitations, Session,
};
use common::{Events, Reply, TestCloud, User};
use serde_json::{Value, json};

async fn befriend(cloud: &TestCloud, a: &User, b: &User) {
    cloud
        .post(
            "/v1/friends/requests",
            a.token(),
            json!({"handle": b.handle}),
        )
        .await
        .ok(200);
    cloud
        .post_empty(&format!("/v1/friends/requests/{}/accept", a.id), b.token())
        .await
        .ok(204);
}

/// A leader with `n` friends, none of them friends with each other.
async fn leader_and_friends(cloud: &TestCloud, n: usize) -> (User, Vec<User>) {
    let leader = cloud.sign_up("Lead").await;
    let mut friends = Vec::new();
    for k in 0..n {
        let friend = cloud.sign_up(&format!("Mate{k}")).await;
        befriend(cloud, &leader, &friend).await;
        friends.push(friend);
    }
    (leader, friends)
}

fn ids(users: &[&User]) -> Vec<u64> {
    users.iter().map(|u| u.id).collect()
}

async fn invite(cloud: &TestCloud, from: &User, to: &[&User]) -> Reply {
    cloud
        .post(
            "/v1/party/invitations",
            from.token(),
            json!({"to": ids(to)}),
        )
        .await
}

async fn invitation_event(events: &mut Events) -> PartyInvitation {
    match events
        .until(|e| matches!(e, Event::PartyInvitation { .. }))
        .await
    {
        Event::PartyInvitation { invitation } => invitation,
        _ => unreachable!(),
    }
}

async fn party_event(events: &mut Events) -> Party {
    match events.until(|e| matches!(e, Event::Party { .. })).await {
        Event::Party { party } => party,
        _ => unreachable!(),
    }
}

async fn party_event_with(events: &mut Events, members: usize) -> Party {
    loop {
        let party = party_event(events).await;
        if party.members.len() == members {
            return party;
        }
    }
}

/// Invites `to`, and has each of them accept the invitation that reaches them.
async fn gather(cloud: &TestCloud, leader: &User, to: &[&User]) -> Party {
    let mut events = Vec::new();
    for user in to {
        events.push(cloud.events(user.token()).await);
    }
    invite(cloud, leader, to).await.ok(200);
    let mut party = None;
    for (user, events) in to.iter().zip(events.iter_mut()) {
        let invitation = invitation_event(events).await;
        party = Some(
            cloud
                .post_empty(
                    &format!("/v1/party/invitations/{}/accept", invitation.id),
                    user.token(),
                )
                .await
                .ok(200)
                .json::<Party>(),
        );
    }
    party.expect("someone accepted")
}

async fn party_of(cloud: &TestCloud, user: &User) -> Reply {
    cloud.get("/v1/party", user.token()).await
}

async fn say(cloud: &TestCloud, user: &User, conversation: &str, text: &str) -> Reply {
    cloud
        .post(
            &format!("/v1/conversations/{conversation}/messages"),
            user.token(),
            json!({"text": text}),
        )
        .await
}

async fn heard(cloud: &TestCloud, user: &User, conversation: &str) -> Vec<String> {
    let messages: Messages = cloud
        .get(
            &format!("/v1/conversations/{conversation}/messages"),
            user.token(),
        )
        .await
        .ok(200)
        .json();
    messages.messages.into_iter().map(|m| m.text).collect()
}

fn endpoint(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

async fn open_session(cloud: &TestCloud, user: &User, visibility: &str) -> Session {
    cloud
        .post(
            "/v1/sessions",
            user.token(),
            json!({"title_id": "GAME00001", "title": "Test Game", "version": "01.000",
                   "build": "0.1.3", "max_members": 8, "visibility": visibility,
                   "properties": "", "endpoint": endpoint(1)}),
        )
        .await
        .ok(200)
        .json()
}

#[tokio::test]
async fn a_party_is_started_by_inviting_and_the_invited_accept_or_decline() {
    let cloud = TestCloud::start().await;
    let (lead, friends) = leader_and_friends(&cloud, 2).await;
    let (bea, cy) = (&friends[0], &friends[1]);
    let mut eb = cloud.events(bea.token()).await;
    let mut ec = cloud.events(cy.token()).await;

    // No party yet.
    party_of(&cloud, &lead).await.is_error("not_found", 404);

    let sent: Invited = invite(&cloud, &lead, &[bea, cy]).await.ok(200).json();
    assert_eq!(sent.sent, [AccountId(bea.id), AccountId(cy.id)]);
    let to_bea = invitation_event(&mut eb).await;
    let to_cy = invitation_event(&mut ec).await;
    assert_eq!(to_bea.from.handle, "Lead");
    assert_eq!(to_bea.members, 1);

    let party: Party = party_of(&cloud, &lead).await.ok(200).json();
    assert_eq!(party.leader, AccountId(lead.id));
    assert_eq!(party.max_members, MAX_PARTY_MEMBERS);
    assert_eq!(party.members.len(), 1);

    // Invitations can be read again after a break.
    let held: PartyInvitations = cloud
        .get("/v1/party/invitations", bea.token())
        .await
        .ok(200)
        .json();
    assert_eq!(held.invitations.len(), 1);
    assert_eq!(held.invitations[0].id, to_bea.id);

    // Cy declines; Bea accepts and everyone in the party hears of it.
    let mut el = cloud.events(lead.token()).await;
    cloud
        .delete(&format!("/v1/party/invitations/{}", to_cy.id), cy.token())
        .await
        .ok(204);
    assert!(matches!(
        ec.until(|e| matches!(e, Event::PartyInvitationGone { .. })).await,
        Event::PartyInvitationGone { id } if id == to_cy.id
    ));
    cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", to_cy.id),
            cy.token(),
        )
        .await
        .is_error("not_found", 404);

    let joined: Party = cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", to_bea.id),
            bea.token(),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(joined.id, party.id);
    assert_eq!(joined.members.len(), 2);
    assert!(joined.revision > party.revision);
    let told = party_event_with(&mut el, 2).await;
    assert_eq!(told.revision, joined.revision);
    assert_eq!(told.members[1].account.handle, "Mate0");
    // An accepted invitation is used up on every device.
    assert!(matches!(
        eb.until(|e| matches!(e, Event::PartyInvitationGone { .. })).await,
        Event::PartyInvitationGone { id } if id == to_bea.id
    ));
    // The invitation was not Bea's to give to someone else, and the party is not a session.
    cloud
        .get("/v1/party", cy.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn only_friends_are_invited_and_only_the_leader_invites() {
    let cloud = TestCloud::start().await;
    let (lead, friends) = leader_and_friends(&cloud, 2).await;
    let stranger = cloud.sign_up("Stranger").await;

    // A stranger gets nothing, and the request still succeeds without them.
    let sent: Invited = invite(&cloud, &lead, &[&stranger, &friends[0]])
        .await
        .ok(200)
        .json();
    assert_eq!(sent.sent, [AccountId(friends[0].id)]);
    let none: PartyInvitations = cloud
        .get("/v1/party/invitations", stranger.token())
        .await
        .ok(200)
        .json();
    assert!(none.invitations.is_empty());

    let held: PartyInvitations = cloud
        .get("/v1/party/invitations", friends[0].token())
        .await
        .ok(200)
        .json();
    let party: Party = cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", held.invitations[0].id),
            friends[0].token(),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(party.members.len(), 2);
    // A member who is not the leader cannot invite, kick or change the party.
    invite(&cloud, &friends[0], &[&friends[1]])
        .await
        .is_error("forbidden", 403);
    cloud
        .delete(
            &format!("/v1/party/members/{}", lead.id),
            friends[0].token(),
        )
        .await
        .is_error("forbidden", 403);
    cloud
        .patch(
            "/v1/party",
            friends[0].token(),
            json!({"leader": friends[0].id}),
        )
        .await
        .is_error("forbidden", 403);
    // The leader can neither kick itself nor hand over to someone outside.
    cloud
        .delete(&format!("/v1/party/members/{}", lead.id), lead.token())
        .await
        .is_error("bad_request", 400);
    cloud
        .patch("/v1/party", lead.token(), json!({"leader": friends[1].id}))
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn a_party_has_eight_members_and_a_player_is_in_one_party() {
    let cloud = TestCloud::start().await;
    let (lead, friends) = leader_and_friends(&cloud, 9).await;
    let refs: Vec<&User> = friends.iter().collect();
    let party = gather(&cloud, &lead, &refs[..7]).await;
    assert_eq!(party.members.len(), 8);

    // Full: no more invitations.
    invite(&cloud, &lead, &[refs[7]])
        .await
        .is_error("full", 409);

    // A player in a party cannot start another.
    cloud
        .post_empty("/v1/party", friends[0].token())
        .await
        .is_error("limit", 409);

    // A seat frees up and an invitation goes out. Its recipient starts a party of its own
    // meanwhile, and cannot accept until it has left that one.
    cloud
        .post_empty("/v1/party/leave", friends[0].token())
        .await
        .ok(204);
    invite(&cloud, &lead, &[refs[7]]).await.ok(200);
    let held: PartyInvitations = cloud
        .get("/v1/party/invitations", refs[7].token())
        .await
        .ok(200)
        .json();
    let accept = format!("/v1/party/invitations/{}/accept", held.invitations[0].id);
    cloud.post_empty("/v1/party", refs[7].token()).await.ok(200);
    cloud
        .post_empty(&accept, refs[7].token())
        .await
        .is_error("limit", 409);
    cloud
        .post_empty("/v1/party/leave", refs[7].token())
        .await
        .ok(204);
    cloud.post_empty(&accept, refs[7].token()).await.ok(200);
    let party: Party = party_of(&cloud, &lead).await.ok(200).json();
    assert_eq!(party.members.len(), 8);
    // The ninth player stays out.
    invite(&cloud, &lead, &[refs[8]])
        .await
        .is_error("full", 409);
}

#[tokio::test]
async fn a_full_party_refuses_an_invitation_sent_before_it_filled() {
    let cloud = TestCloud::start().await;
    let (lead, friends) = leader_and_friends(&cloud, 9).await;
    let refs: Vec<&User> = friends.iter().collect();
    // Seven are in. Two more invitations go out, and only one seat is left for them.
    gather(&cloud, &lead, &refs[..6]).await;
    invite(&cloud, &lead, &[refs[6], refs[7]]).await.ok(200);
    let waiting: PartyInvitations = cloud
        .get("/v1/party/invitations", refs[7].token())
        .await
        .ok(200)
        .json();
    let late = &waiting.invitations[0];
    let early: PartyInvitations = cloud
        .get("/v1/party/invitations", refs[6].token())
        .await
        .ok(200)
        .json();
    cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", early.invitations[0].id),
            refs[6].token(),
        )
        .await
        .ok(200);
    // The first to accept takes the last seat.
    assert_eq!(
        party_of(&cloud, &lead)
            .await
            .ok(200)
            .json::<Party>()
            .members
            .len(),
        8
    );
    cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", late.id),
            refs[7].token(),
        )
        .await
        .is_error("full", 409);
}

#[tokio::test]
async fn the_leader_hands_over_and_a_leaving_leader_hands_over_too() {
    let cloud = TestCloud::start().await;
    let (lead, friends) = leader_and_friends(&cloud, 2).await;
    gather(&cloud, &lead, &[&friends[0]]).await;
    let mut eb = cloud.events(friends[0].token()).await;

    // Passing the lead on.
    let party: Party = cloud
        .patch("/v1/party", lead.token(), json!({"leader": friends[0].id}))
        .await
        .ok(200)
        .json();
    assert_eq!(party.leader, AccountId(friends[0].id));
    let told = party_event(&mut eb).await;
    assert_eq!(told.leader, AccountId(friends[0].id));
    assert_eq!(told.revision, party.revision);

    // The new leader leaves: the longest-standing member, Lead, leads again.
    cloud
        .post_empty("/v1/party/leave", friends[0].token())
        .await
        .ok(204);
    let party: Party = party_of(&cloud, &lead).await.ok(200).json();
    assert_eq!(party.leader, AccountId(lead.id));
    assert_eq!(party.members.len(), 1);
    assert!(matches!(
        eb.until(|e| matches!(e, Event::PartyGone { .. })).await,
        Event::PartyGone { id } if id == party.id
    ));
    party_of(&cloud, &friends[0])
        .await
        .is_error("not_found", 404);

    // The last member leaves and the party ends, with its chat.
    cloud
        .post_empty("/v1/party/leave", lead.token())
        .await
        .ok(204);
    party_of(&cloud, &lead).await.is_error("not_found", 404);
    cloud
        .get(
            &format!("/v1/conversations/{}", party.conversation),
            lead.token(),
        )
        .await
        .is_error("not_found", 404);
    cloud
        .post_empty("/v1/party/leave", lead.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn the_leader_removes_a_member() {
    let cloud = TestCloud::start().await;
    let (lead, friends) = leader_and_friends(&cloud, 2).await;
    gather(&cloud, &lead, &[&friends[0], &friends[1]]).await;
    let mut eb = cloud.events(friends[0].token()).await;
    let mut el = cloud.events(lead.token()).await;

    cloud
        .delete(
            &format!("/v1/party/members/{}", friends[0].id),
            lead.token(),
        )
        .await
        .ok(204);
    assert!(matches!(
        eb.until(|e| matches!(e, Event::PartyGone { .. })).await,
        Event::PartyGone { .. }
    ));
    let told = party_event_with(&mut el, 2).await;
    assert!(told.members.iter().all(|m| m.account.id.0 != friends[0].id));
    cloud
        .delete(
            &format!("/v1/party/members/{}", friends[0].id),
            lead.token(),
        )
        .await
        .is_error("not_found", 404);
    party_of(&cloud, &friends[0])
        .await
        .is_error("not_found", 404);
    // The removed player can be invited again.
    invite(&cloud, &lead, &[&friends[0]]).await.ok(200);
}

#[tokio::test]
async fn a_block_keeps_a_player_out_of_the_party_and_ends_invitations() {
    let cloud = TestCloud::start().await;
    let (lead, friends) = leader_and_friends(&cloud, 2).await;
    let (bea, cy) = (&friends[0], &friends[1]);
    befriend(&cloud, bea, cy).await;
    gather(&cloud, &lead, &[bea]).await;

    // Cy holds an invitation, then blocks a member: it cannot be accepted any more.
    invite(&cloud, &lead, &[cy]).await.ok(200);
    let held: PartyInvitations = cloud
        .get("/v1/party/invitations", cy.token())
        .await
        .ok(200)
        .json();
    cloud
        .put(&format!("/v1/blocks/{}", bea.id), cy.token(), None)
        .await
        .ok(204);
    cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", held.invitations[0].id),
            cy.token(),
        )
        .await
        .is_error("forbidden", 403);
    // A later invitation to someone who blocked a member is not sent at all.
    let sent: Invited = invite(&cloud, &lead, &[cy]).await.ok(200).json();
    assert!(sent.sent.is_empty());

    // Blocking the sender of an invitation ends it at once.
    let dee = cloud.sign_up("Dee").await;
    befriend(&cloud, &lead, &dee).await;
    let mut ed = cloud.events(dee.token()).await;
    invite(&cloud, &lead, &[&dee]).await.ok(200);
    let invitation = invitation_event(&mut ed).await;
    cloud
        .put(&format!("/v1/blocks/{}", lead.id), dee.token(), None)
        .await
        .ok(204);
    assert!(matches!(
        ed.until(|e| matches!(e, Event::PartyInvitationGone { .. })).await,
        Event::PartyInvitationGone { id } if id == invitation.id
    ));
}

#[tokio::test]
async fn an_invitation_runs_out() {
    let cloud = TestCloud::with(|_| {}, |t| t.invitation_ttl = Duration::from_millis(700)).await;
    let (lead, friends) = leader_and_friends(&cloud, 1).await;
    let mut eb = cloud.events(friends[0].token()).await;
    invite(&cloud, &lead, &[&friends[0]]).await.ok(200);
    let invitation = invitation_event(&mut eb).await;
    assert!(matches!(
        eb.until(|e| matches!(e, Event::PartyInvitationGone { .. })).await,
        Event::PartyInvitationGone { id } if id == invitation.id
    ));
    cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", invitation.id),
            friends[0].token(),
        )
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn a_member_who_stays_away_leaves_the_party_but_one_online_stays() {
    let cloud = TestCloud::with(|_| {}, |t| t.party_grace = Duration::from_millis(900)).await;
    let (lead, friends) = leader_and_friends(&cloud, 1).await;
    let _el = cloud.events(lead.token()).await;
    gather(&cloud, &lead, &[&friends[0]]).await;
    // Bea's event connection from `gather` is dropped; hers is the only one that ended.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let party: Party = party_of(&cloud, &lead).await.ok(200).json();
    assert_eq!(party.members.len(), 1, "the absent member left");
    assert_eq!(party.leader, AccountId(lead.id));
}

#[tokio::test]
async fn banning_or_deleting_an_account_takes_it_out_of_its_party() {
    let cloud = TestCloud::with(
        |c| c.admin_token = Some("an-admin-token-for-tests".into()),
        |_| {},
    )
    .await;
    let (lead, friends) = leader_and_friends(&cloud, 2).await;
    gather(&cloud, &lead, &[&friends[0], &friends[1]]).await;

    cloud.delete("/v1/me", friends[0].token()).await.ok(204);
    let party: Party = party_of(&cloud, &lead).await.ok(200).json();
    assert_eq!(party.members.len(), 2);
    cloud
        .request(
            reqwest::Method::POST,
            "/admin/v1/bans",
            Some("an-admin-token-for-tests"),
            Some(json!({"handle": friends[1].handle, "reason": "cheating"})),
        )
        .await
        .ok(200);
    let party: Party = party_of(&cloud, &lead).await.ok(200).json();
    assert_eq!(party.members.len(), 1);
}

#[tokio::test]
async fn the_party_has_its_own_chat_that_starts_when_a_member_joins() {
    let cloud = TestCloud::start().await;
    let (lead, friends) = leader_and_friends(&cloud, 2).await;
    let party = gather(&cloud, &lead, &[&friends[0]]).await;
    let chat = party.conversation.clone();

    say(&cloud, &lead, &chat, "welcome").await.ok(200);
    say(&cloud, &friends[0], &chat, "hi").await.ok(200);

    // Cy joins later and sees what is said from then on.
    let mut ec = cloud.events(friends[1].token()).await;
    invite(&cloud, &lead, &[&friends[1]]).await.ok(200);
    let invitation = invitation_event(&mut ec).await;
    cloud
        .post_empty(
            &format!("/v1/party/invitations/{}/accept", invitation.id),
            friends[1].token(),
        )
        .await
        .ok(200);
    say(&cloud, &lead, &chat, "all here").await.ok(200);
    assert_eq!(heard(&cloud, &friends[1], &chat).await, ["all here"]);
    assert_eq!(
        heard(&cloud, &friends[0], &chat).await,
        ["welcome", "hi", "all here"]
    );
    let conversation: Conversation = cloud
        .get(&format!("/v1/conversations/{chat}"), friends[1].token())
        .await
        .ok(200)
        .json();
    assert_eq!(conversation.party.as_deref(), Some(party.id.as_str()));
    assert_eq!(conversation.members.len(), 3);

    // A member who left can neither read nor write; the chat cannot be left on its own.
    cloud
        .post_empty("/v1/party/leave", friends[0].token())
        .await
        .ok(204);
    say(&cloud, &friends[0], &chat, "still there?")
        .await
        .is_error("not_found", 404);
    cloud
        .get(
            &format!("/v1/conversations/{chat}/messages"),
            friends[0].token(),
        )
        .await
        .is_error("not_found", 404);
    cloud
        .delete(&format!("/v1/conversations/{chat}"), lead.token())
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn the_leader_moves_the_party_into_a_session_with_invitations() {
    let cloud = TestCloud::start().await;
    let (lead, friends) = leader_and_friends(&cloud, 2).await;
    let (bea, cy) = (&friends[0], &friends[1]);
    gather(&cloud, &lead, &[bea, cy]).await;
    let mut eb = cloud.events(bea.token()).await;
    let mut ec = cloud.events(cy.token()).await;

    // Not the leader, not in the session, or no session at all.
    let session = open_session(&cloud, &lead, "invite").await;
    cloud
        .post(
            "/v1/party/session",
            bea.token(),
            json!({"session": session.id}),
        )
        .await
        .is_error("forbidden", 403);
    let elsewhere = open_session(&cloud, bea, "invite").await;
    cloud
        .post(
            "/v1/party/session",
            lead.token(),
            json!({"session": elsewhere.id}),
        )
        .await
        .is_error("not_found", 404);
    cloud
        .post(
            "/v1/party/session",
            lead.token(),
            json!({"session": "nope"}),
        )
        .await
        .is_error("not_found", 404);

    // Everyone else gets an invitation to the leader's session, even a session for invited
    // players only.
    let invited: Invited = cloud
        .post(
            "/v1/party/session",
            lead.token(),
            json!({"session": session.id}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(invited.sent.len(), 2);
    let to_bea = match eb.until(|e| matches!(e, Event::Invitation { .. })).await {
        Event::Invitation { invitation } => invitation,
        _ => unreachable!(),
    };
    let _: Invitation = match ec.until(|e| matches!(e, Event::Invitation { .. })).await {
        Event::Invitation { invitation } => invitation,
        _ => unreachable!(),
    };
    assert_eq!(to_bea.session, session.id);
    assert_eq!(to_bea.from.id, AccountId(lead.id));

    cloud
        .post(
            &format!("/v1/sessions/{}/join", session.id),
            bea.token(),
            json!({"version": "01.000", "build": "0.1.3", "endpoint": endpoint(2),
                   "invitation": to_bea.id}),
        )
        .await
        .ok(200);
    // Moving again invites only those who are not in yet.
    let invited: Invited = cloud
        .post(
            "/v1/party/session",
            lead.token(),
            json!({"session": session.id}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(invited.sent, [AccountId(cy.id)]);

    // A closed session takes no one.
    let closed = open_session(&cloud, &lead, "closed").await;
    cloud
        .post(
            "/v1/party/session",
            lead.token(),
            json!({"session": closed.id}),
        )
        .await
        .is_error("forbidden", 403);
    // The party itself stays as it was.
    let party: Party = party_of(&cloud, &lead).await.ok(200).json();
    assert_eq!(party.members.len(), 3);
}

/// The goal of S2: three players form a party, chat and move into one session together.
#[tokio::test]
async fn a_party_of_three_chats_and_moves_into_one_session() {
    let cloud = TestCloud::start().await;
    let (alec, friends) = leader_and_friends(&cloud, 2).await;
    let (bea, cy) = (&friends[0], &friends[1]);
    let mut ea = cloud.events(alec.token()).await;
    let mut eb = cloud.events(bea.token()).await;
    let mut ec = cloud.events(cy.token()).await;

    // Alec invites Bea and Cy; both accept.
    invite(&cloud, &alec, &[bea, cy]).await.ok(200);
    for (user, events) in [(bea, &mut eb), (cy, &mut ec)] {
        let invitation = invitation_event(events).await;
        cloud
            .post_empty(
                &format!("/v1/party/invitations/{}/accept", invitation.id),
                user.token(),
            )
            .await
            .ok(200);
    }
    let party = party_event_with(&mut ea, 3).await;
    assert_eq!(party.members.len(), 3);

    // They chat. Each hears a pointer and fetches the message.
    let chat = party.conversation.clone();
    let sent: Message = say(&cloud, bea, &chat, "ready?").await.ok(200).json();
    assert_eq!(sent.seq, 1);
    for events in [&mut ea, &mut ec] {
        match events.until(|e| matches!(e, Event::Message { .. })).await {
            Event::Message { conversation, seq } => {
                assert_eq!((conversation.as_str(), seq), (chat.as_str(), 1));
            }
            _ => unreachable!(),
        }
    }
    say(&cloud, cy, &chat, "ready").await.ok(200);
    say(&cloud, &alec, &chat, "let's go").await.ok(200);
    assert_eq!(
        heard(&cloud, bea, &chat).await,
        ["ready?", "ready", "let's go"]
    );

    // Alec starts a session and moves the party into it; both others join.
    let session = open_session(&cloud, &alec, "friends").await;
    cloud
        .post(
            "/v1/party/session",
            alec.token(),
            json!({"session": session.id}),
        )
        .await
        .ok(200);
    for (n, (user, events)) in [(bea, &mut eb), (cy, &mut ec)].into_iter().enumerate() {
        let invitation = match events
            .until(|e| matches!(e, Event::Invitation { .. }))
            .await
        {
            Event::Invitation { invitation } => invitation,
            _ => unreachable!(),
        };
        cloud
            .post(
                &format!("/v1/sessions/{}/join", invitation.session),
                user.token(),
                json!({"version": "01.000", "build": "0.1.3",
                       "endpoint": endpoint(10 + n as u8), "invitation": invitation.id}),
            )
            .await
            .ok(200);
    }
    let session: Session = cloud
        .get(&format!("/v1/sessions/{}", session.id), alec.token())
        .await
        .ok(200)
        .json();
    assert_eq!(session.members.len(), 3);
    // The party is still together and still talks.
    let party: Party = party_of(&cloud, &alec).await.ok(200).json();
    assert_eq!(party.members.len(), 3);
    say(&cloud, bea, &chat, "in").await.ok(200);
    let _: Value = cloud
        .get(
            &format!("/v1/conversations/{chat}/messages?after=3"),
            cy.token(),
        )
        .await
        .ok(200)
        .body;
}
