//! Replies, edits, unsends, reactions, typing, mute and pin, unread counts and delivered marks
//! (`docs/adr/0034` in the network repository). Runs on SQLite, and on Postgres when
//! `ANYPLAY_TEST_DATABASE_URL` is set.

mod common;

use std::time::Duration;

use anyplay_api::{Conversation, Event, Message, MessageReaction, Messages};
use common::{Events, Reply, TestCloud, User};
use serde_json::{Value, json};

const QUIET: Duration = Duration::from_millis(300);
const ADMIN: &str = "test-admin-token";

async fn cloud() -> TestCloud {
    TestCloud::with(
        |c| {
            c.new_account_hours = 0;
            c.admin_token = Some(ADMIN.into());
        },
        |_| {},
    )
    .await
}

async fn open(cloud: &TestCloud, from: &User, with: &[&User]) -> Conversation {
    cloud
        .post(
            "/v1/conversations",
            from.token(),
            json!({"accounts": with.iter().map(|u| u.id).collect::<Vec<_>>()}),
        )
        .await
        .ok(200)
        .json()
}

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

async fn say(cloud: &TestCloud, user: &User, conversation: &str, text: &str) -> Message {
    cloud
        .post(
            &format!("/v1/conversations/{conversation}/messages"),
            user.token(),
            json!({"text": text}),
        )
        .await
        .ok(200)
        .json()
}

async fn answer(cloud: &TestCloud, user: &User, conversation: &str, text: &str, to: u64) -> Reply {
    cloud
        .post(
            &format!("/v1/conversations/{conversation}/messages"),
            user.token(),
            json!({"text": text, "reply": to}),
        )
        .await
}

async fn fetch(cloud: &TestCloud, user: &User, conversation: &str, query: &str) -> Messages {
    cloud
        .get(
            &format!("/v1/conversations/{conversation}/messages{query}"),
            user.token(),
        )
        .await
        .ok(200)
        .json()
}

async fn edit(cloud: &TestCloud, user: &User, conversation: &str, seq: u64, text: &str) -> Reply {
    cloud
        .patch(
            &format!("/v1/conversations/{conversation}/messages/{seq}"),
            user.token(),
            json!({"text": text}),
        )
        .await
}

async fn unsend(cloud: &TestCloud, user: &User, conversation: &str, seq: u64) -> Reply {
    cloud
        .delete(
            &format!("/v1/conversations/{conversation}/messages/{seq}"),
            user.token(),
        )
        .await
}

async fn react(
    cloud: &TestCloud,
    user: &User,
    conversation: &str,
    seq: u64,
    reaction: &str,
) -> Reply {
    cloud
        .put(
            &format!("/v1/conversations/{conversation}/messages/{seq}/reaction"),
            user.token(),
            Some(json!({"reaction": reaction})),
        )
        .await
}

async fn unreact(cloud: &TestCloud, user: &User, conversation: &str, seq: u64) -> Reply {
    cloud
        .delete(
            &format!("/v1/conversations/{conversation}/messages/{seq}/reaction"),
            user.token(),
        )
        .await
}

async fn signal_typing(cloud: &TestCloud, user: &User, conversation: &str, on: bool) {
    cloud
        .put(
            &format!("/v1/conversations/{conversation}/typing"),
            user.token(),
            Some(json!({"typing": on})),
        )
        .await
        .ok(204);
}

async fn settings(cloud: &TestCloud, user: &User, conversation: &str, body: Value) -> Reply {
    cloud
        .patch(
            &format!("/v1/conversations/{conversation}/settings"),
            user.token(),
            body,
        )
        .await
}

async fn conversation(cloud: &TestCloud, user: &User, id: &str) -> Conversation {
    cloud
        .get(&format!("/v1/conversations/{id}"), user.token())
        .await
        .ok(200)
        .json()
}

async fn age_message(cloud: &TestCloud, conversation: &str, seq: u64, minutes: i64) {
    sqlx::query(
        "UPDATE messages SET created_ms = created_ms - $3 WHERE conversation_id = $1 AND seq = $2",
    )
    .bind(conversation)
    .bind(seq as i64)
    .bind(minutes * 60 * 1000)
    .execute(&cloud.state.db)
    .await
    .unwrap();
}

async fn nothing_like(events: &mut Events, unwanted: impl Fn(&Event) -> bool) {
    let deadline = tokio::time::Instant::now() + QUIET;
    while let Some(event) = events
        .next_within(deadline.saturating_duration_since(tokio::time::Instant::now()))
        .await
    {
        assert!(!unwanted(&event), "unexpected {event:?}");
    }
}

#[tokio::test]
async fn a_reply_quotes_the_message_it_answers() {
    let cloud = cloud().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    let first = say(&cloud, &alec, &chat.id, &"long ".repeat(60)).await;

    let reply: Message = answer(&cloud, &bea, &chat.id, "yes", first.seq)
        .await
        .ok(200)
        .json();
    let quote = reply.reply.clone().expect("the answer carries the quote");
    assert_eq!((quote.seq, quote.sender.handle.as_str()), (1, "Alec"));
    assert_eq!(quote.text.chars().count(), 140, "a quote is cut");
    assert!(!quote.deleted);
    // The fetch gives the same quote to the other side.
    let seen = fetch(&cloud, &alec, &chat.id, "").await;
    assert_eq!(seen.messages[1].reply, reply.reply);
    // A plain message carries none.
    assert_eq!(seen.messages[0].reply, None);

    // Only a message of this conversation that exists and that was not unsent.
    answer(&cloud, &bea, &chat.id, "x", 99)
        .await
        .is_error("not_found", 404);
    unsend(&cloud, &alec, &chat.id, 1).await.ok(200);
    answer(&cloud, &bea, &chat.id, "x", 1)
        .await
        .is_error("bad_request", 400);
    // The old reply now quotes the marker.
    let after = fetch(&cloud, &bea, &chat.id, "").await;
    let quote = after.messages[1].reply.clone().unwrap();
    assert!(quote.deleted);
    assert_eq!(quote.text, "");
}

#[tokio::test]
async fn a_member_who_joined_later_gets_no_quote_of_earlier_messages() {
    let cloud = cloud().await;
    let (hub, friends) = {
        let hub = cloud.sign_up("Hub").await;
        let mut friends = Vec::new();
        for name in ["Pal0", "Pal1", "Pal2"] {
            let friend = cloud.sign_up(name).await;
            befriend(&cloud, &hub, &friend).await;
            friends.push(friend);
        }
        (hub, friends)
    };
    let group = open(&cloud, &hub, &[&friends[0], &friends[1]]).await;
    let old = say(&cloud, &hub, &group.id, "secret from before").await;
    cloud
        .post(
            &format!("/v1/conversations/{}/members", group.id),
            hub.token(),
            json!({"accounts": [friends[2].id]}),
        )
        .await
        .ok(200);
    let new = answer(&cloud, &friends[0], &group.id, "answering", old.seq)
        .await
        .ok(200)
        .json::<Message>();
    let late = fetch(&cloud, &friends[2], &group.id, "").await;
    assert_eq!(late.messages.len(), 1);
    assert_eq!(late.messages[0].seq, new.seq);
    assert_eq!(late.messages[0].reply, None, "no word of what came before");
    // And the late member cannot answer, react to or edit what it cannot see.
    answer(&cloud, &friends[2], &group.id, "x", old.seq)
        .await
        .is_error("not_found", 404);
    react(&cloud, &friends[2], &group.id, old.seq, "gg")
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn the_writer_edits_for_fifteen_minutes_and_the_message_says_so() {
    let cloud = cloud().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    let mut eb = cloud.events(bea.token()).await;
    let one = say(&cloud, &alec, &chat.id, "helo").await;
    assert_eq!(conversation(&cloud, &bea, &chat.id).await.rev, 0);

    let edited: Message = edit(&cloud, &alec, &chat.id, one.seq, "hello")
        .await
        .ok(200)
        .json();
    assert_eq!(edited.text, "hello");
    assert!(edited.edited.is_some());
    assert_eq!(edited.rev, 1);
    // The reader is told by a pointer and fetches the change.
    let Event::MessageUpdate { seq, rev, .. } =
        eb.until(|e| matches!(e, Event::MessageUpdate { .. })).await
    else {
        unreachable!()
    };
    assert_eq!((seq, rev), (1, 1));
    let changed = fetch(&cloud, &bea, &chat.id, "?changed_after=0").await;
    assert_eq!((changed.messages.len(), changed.rev), (1, 1));
    assert_eq!(changed.messages[0].text, "hello");
    assert!(
        fetch(&cloud, &bea, &chat.id, "?changed_after=1")
            .await
            .messages
            .is_empty()
    );

    // The same text again changes nothing and counts for nothing.
    edit(&cloud, &alec, &chat.id, one.seq, "hello")
        .await
        .ok(200);
    assert_eq!(conversation(&cloud, &bea, &chat.id).await.rev, 1);

    // Not someone else's message, not nothing, not after fifteen minutes.
    edit(&cloud, &bea, &chat.id, one.seq, "mine now")
        .await
        .is_error("forbidden", 403);
    edit(&cloud, &alec, &chat.id, one.seq, "  ")
        .await
        .is_error("bad_request", 400);
    age_message(&cloud, &chat.id, one.seq, 16).await;
    edit(&cloud, &alec, &chat.id, one.seq, "too late")
        .await
        .is_error("forbidden", 403);
    assert_eq!(
        fetch(&cloud, &bea, &chat.id, "").await.messages[0].text,
        "hello"
    );
    // Editing does not move the conversation up the list.
    let listed = conversation(&cloud, &bea, &chat.id).await;
    assert_eq!(listed.last, 1);
}

#[tokio::test]
async fn unsending_removes_the_text_for_everyone_and_keeps_the_number() {
    let cloud = cloud().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    let one = say(&cloud, &alec, &chat.id, "I regret this").await;
    say(&cloud, &bea, &chat.id, "two").await;
    react(&cloud, &bea, &chat.id, one.seq, "wow").await.ok(200);

    unsend(&cloud, &bea, &chat.id, one.seq)
        .await
        .is_error("forbidden", 403);
    let gone: Message = unsend(&cloud, &alec, &chat.id, one.seq)
        .await
        .ok(200)
        .json();
    assert!(gone.deleted);
    assert_eq!(gone.text, "");
    assert!(gone.reactions.is_empty());

    let seen = fetch(&cloud, &bea, &chat.id, "").await;
    assert_eq!(
        seen.messages.iter().map(|m| m.seq).collect::<Vec<_>>(),
        [1, 2],
        "the number stays"
    );
    assert!(seen.messages[0].deleted && seen.messages[0].text.is_empty());
    // The store holds no text and no reaction any more.
    let (body, reactions): (String, i64) = (
        sqlx::query_as::<_, (String,)>("SELECT body FROM messages WHERE seq = 1")
            .fetch_one(&cloud.state.db)
            .await
            .unwrap()
            .0,
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM message_reactions")
            .fetch_one(&cloud.state.db)
            .await
            .unwrap()
            .0,
    );
    assert_eq!((body.as_str(), reactions), ("", 0));

    // A marker cannot be edited, reacted to or unsent again for another revision.
    edit(&cloud, &alec, &chat.id, one.seq, "back")
        .await
        .is_error("bad_request", 400);
    react(&cloud, &bea, &chat.id, one.seq, "gg")
        .await
        .is_error("bad_request", 400);
    let rev = conversation(&cloud, &bea, &chat.id).await.rev;
    unsend(&cloud, &alec, &chat.id, one.seq).await.ok(200);
    assert_eq!(conversation(&cloud, &bea, &chat.id).await.rev, rev);
    // Unread no longer counts it.
    let unread = conversation(&cloud, &bea, &chat.id).await.unread;
    assert_eq!(unread, 0, "the only message of the other side is unsent");
}

#[tokio::test]
async fn one_reaction_per_account_from_a_fixed_set() {
    let cloud = cloud().await;
    let hub = cloud.sign_up("Hub").await;
    let (bea, cy) = (cloud.sign_up("Bea").await, cloud.sign_up("Cyd").await);
    befriend(&cloud, &hub, &bea).await;
    befriend(&cloud, &hub, &cy).await;
    let group = open(&cloud, &hub, &[&bea, &cy]).await;
    let one = say(&cloud, &hub, &group.id, "gg everyone").await;

    let first: Message = react(&cloud, &bea, &group.id, one.seq, "gg")
        .await
        .ok(200)
        .json();
    assert_eq!(first.reactions.len(), 1);
    assert_eq!(first.reactions[0].reaction, MessageReaction::Gg);
    react(&cloud, &cy, &group.id, one.seq, "gg").await.ok(200);
    // A new one replaces the old.
    let changed: Message = react(&cloud, &bea, &group.id, one.seq, "love")
        .await
        .ok(200)
        .json();
    let names: Vec<_> = changed.reactions.iter().map(|r| r.reaction).collect();
    assert_eq!(names, [MessageReaction::Love, MessageReaction::Gg]);
    assert_eq!(changed.reactions[1].accounts.len(), 1);
    // The writer sees them in a plain fetch.
    let seen = fetch(&cloud, &hub, &group.id, "").await;
    assert_eq!(seen.messages[0].reactions, changed.reactions);
    // Taking one back.
    let taken: Message = unreact(&cloud, &cy, &group.id, one.seq)
        .await
        .ok(200)
        .json();
    assert_eq!(taken.reactions.len(), 1);
    // Nothing outside the set.
    react(&cloud, &bea, &group.id, one.seq, "thumbs_up")
        .await
        .is_error("bad_request", 400);
    react(&cloud, &bea, &group.id, one.seq, "\u{1F44D}")
        .await
        .is_error("bad_request", 400);
    // Every change is a revision.
    assert_eq!(conversation(&cloud, &hub, &group.id).await.rev, 4);
}

#[tokio::test]
async fn reactions_of_blocked_accounts_are_left_out_and_requests_cannot_react() {
    let cloud = cloud().await;
    let hub = cloud.sign_up("Hub").await;
    let (bea, cy) = (cloud.sign_up("Bea").await, cloud.sign_up("Cyd").await);
    befriend(&cloud, &hub, &bea).await;
    befriend(&cloud, &hub, &cy).await;
    let group = open(&cloud, &hub, &[&bea, &cy]).await;
    let one = say(&cloud, &hub, &group.id, "hello").await;
    react(&cloud, &bea, &group.id, one.seq, "nice")
        .await
        .ok(200);
    react(&cloud, &cy, &group.id, one.seq, "nice").await.ok(200);
    // Cy blocks Bea: Cy no longer sees Bea's reaction, and Bea no longer sees Cy's.
    cloud
        .put(&format!("/v1/blocks/{}", bea.id), cy.token(), None)
        .await
        .ok(204);
    let for_cy = fetch(&cloud, &cy, &group.id, "").await;
    assert_eq!(for_cy.messages[0].reactions[0].accounts.len(), 1);
    assert_eq!(for_cy.messages[0].reactions[0].accounts[0].0, cy.id);
    let for_hub = fetch(&cloud, &hub, &group.id, "").await;
    assert_eq!(for_hub.messages[0].reactions[0].accounts.len(), 2);

    // A message request cannot be reacted to before it is accepted.
    let stranger = cloud.sign_up("Zed").await;
    let request = open(&cloud, &stranger, &[&hub]).await;
    let note = say(&cloud, &stranger, &request.id, "psst").await;
    react(&cloud, &hub, &request.id, note.seq, "wow")
        .await
        .is_error("forbidden", 403);
}

#[tokio::test]
async fn typing_is_told_to_the_others_and_never_stored() {
    let cloud = cloud().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    let mut ea = cloud.events(alec.token()).await;
    let mut eb = cloud.events(bea.token()).await;

    signal_typing(&cloud, &alec, &chat.id, true).await;
    let Event::Typing {
        account, typing, ..
    } = eb.until(|e| matches!(e, Event::Typing { .. })).await
    else {
        unreachable!()
    };
    assert_eq!((account.0, typing), (alec.id, true));
    // The typist is not told, and a second signal at once is dropped.
    nothing_like(&mut ea, |e| matches!(e, Event::Typing { .. })).await;
    signal_typing(&cloud, &alec, &chat.id, true).await;
    nothing_like(&mut eb, |e| matches!(e, Event::Typing { .. })).await;
    // Stopping always goes through.
    signal_typing(&cloud, &alec, &chat.id, false).await;
    let Event::Typing { typing, .. } = eb.until(|e| matches!(e, Event::Typing { .. })).await else {
        unreachable!()
    };
    assert!(!typing);
    // Nothing of it is in the store or in the numbers.
    assert_eq!(conversation(&cloud, &bea, &chat.id).await.last, 0);
    // A stranger outside the conversation cannot make it up.
    let zed = cloud.sign_up("Zed").await;
    cloud
        .put(
            &format!("/v1/conversations/{}/typing", chat.id),
            zed.token(),
            Some(json!({"typing": true})),
        )
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn nobody_hears_typing_across_a_block_or_from_a_request() {
    let cloud = cloud().await;
    let hub = cloud.sign_up("Hub").await;
    let (bea, cy) = (cloud.sign_up("Bea").await, cloud.sign_up("Cyd").await);
    befriend(&cloud, &hub, &bea).await;
    befriend(&cloud, &hub, &cy).await;
    let group = open(&cloud, &hub, &[&bea, &cy]).await;
    cloud
        .put(&format!("/v1/blocks/{}", bea.id), cy.token(), None)
        .await
        .ok(204);
    let (mut eb, mut ec) = (
        cloud.events(bea.token()).await,
        cloud.events(cy.token()).await,
    );
    let mut eh = cloud.events(hub.token()).await;
    signal_typing(&cloud, &bea, &group.id, true).await;
    eh.until(|e| matches!(e, Event::Typing { .. })).await;
    nothing_like(&mut ec, |e| matches!(e, Event::Typing { .. })).await;
    nothing_like(&mut eb, |e| matches!(e, Event::Typing { .. })).await;

    // A stranger's request: the receiver has not accepted, so nothing goes either way.
    let zed = cloud.sign_up("Zed").await;
    let request = open(&cloud, &zed, &[&hub]).await;
    say(&cloud, &zed, &request.id, "psst").await;
    signal_typing(&cloud, &zed, &request.id, true).await;
    signal_typing(&cloud, &hub, &request.id, true).await;
    nothing_like(&mut eh, |e| matches!(e, Event::Typing { .. })).await;
}

#[tokio::test]
async fn mute_and_pin_are_the_players_own_and_reach_their_devices() {
    let cloud = cloud().await;
    let hub = cloud.sign_up("Hub").await;
    let mut pals = Vec::new();
    for k in 0..6 {
        let pal = cloud.sign_up(&format!("Pal{k}")).await;
        befriend(&cloud, &hub, &pal).await;
        pals.push(pal);
    }
    let mut chats = Vec::new();
    for pal in &pals {
        let chat = open(&cloud, &hub, &[pal]).await;
        say(&cloud, pal, &chat.id, "hi").await;
        chats.push(chat);
    }
    let mut mine = cloud.events(hub.token()).await;

    let muted: Conversation = settings(&cloud, &hub, &chats[0].id, json!({"muted": true}))
        .await
        .ok(200)
        .json();
    assert!(muted.muted && !muted.pinned);
    mine.until(|e| matches!(e, Event::Conversation { id } if *id == chats[0].id))
        .await;
    // The other side sees nothing of it.
    assert!(!conversation(&cloud, &pals[0], &chats[0].id).await.muted);
    // A field left out stays.
    let both: Conversation = settings(&cloud, &hub, &chats[0].id, json!({"pinned": true}))
        .await
        .ok(200)
        .json();
    assert!(both.muted && both.pinned);

    // The oldest chat, pinned, comes first; the muted one keeps its number.
    settings(&cloud, &hub, &chats[0].id, json!({"pinned": true}))
        .await
        .ok(200);
    let listed = cloud
        .get("/v1/conversations", hub.token())
        .await
        .ok(200)
        .json::<anyplay_api::Conversations>();
    assert_eq!(listed.conversations[0].id, chats[0].id);
    assert_eq!(listed.conversations[0].unread, 1, "mute keeps the count");

    // Five pins at most.
    for chat in &chats[1..5] {
        settings(&cloud, &hub, &chat.id, json!({"pinned": true}))
            .await
            .ok(200);
    }
    settings(&cloud, &hub, &chats[5].id, json!({"pinned": true}))
        .await
        .is_error("limit", 409);
    settings(&cloud, &hub, &chats[1].id, json!({"pinned": false}))
        .await
        .ok(200);
    settings(&cloud, &hub, &chats[5].id, json!({"pinned": true}))
        .await
        .ok(200);

    // Not for somebody else's conversation.
    settings(&cloud, &pals[3], &chats[0].id, json!({"muted": true}))
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn unread_is_counted_by_the_cloud_and_is_the_same_on_every_device() {
    let cloud = cloud().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    say(&cloud, &alec, &chat.id, "one").await;
    say(&cloud, &alec, &chat.id, "two").await;
    say(&cloud, &alec, &chat.id, "three").await;
    say(&cloud, &bea, &chat.id, "mine").await;
    let listed = cloud
        .get("/v1/conversations", bea.token())
        .await
        .ok(200)
        .json::<anyplay_api::Conversations>();
    // Writing reads up to the message written, so only what came after is unread.
    assert_eq!(listed.conversations[0].unread, 0);
    say(&cloud, &alec, &chat.id, "four").await;
    say(&cloud, &alec, &chat.id, "five").await;
    assert_eq!(conversation(&cloud, &bea, &chat.id).await.unread, 2);
    assert_eq!(conversation(&cloud, &alec, &chat.id).await.unread, 0);

    // One message unsent: the count drops, with no read at all.
    unsend(&cloud, &alec, &chat.id, 5).await.ok(200);
    assert_eq!(conversation(&cloud, &bea, &chat.id).await.unread, 1);

    // Reading on one device is what the other devices see.
    cloud
        .put(
            &format!("/v1/conversations/{}/read", chat.id),
            bea.token(),
            Some(json!({"seq": 6})),
        )
        .await
        .ok(204);
    assert_eq!(conversation(&cloud, &bea, &chat.id).await.unread, 0);
    let listed = cloud
        .get("/v1/conversations", bea.token())
        .await
        .ok(200)
        .json::<anyplay_api::Conversations>();
    assert_eq!(listed.conversations[0].unread, 0);
}

#[tokio::test]
async fn the_unread_count_stops_at_the_cap() {
    let cloud = cloud().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    say(&cloud, &alec, &chat.id, "first").await;
    // A long unread history, written straight to the store (the send limit would take minutes).
    // Recent, or the retention sweep would take the rows away mid-test.
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    for seq in 2..=150_i64 {
        sqlx::query(
            "INSERT INTO messages (conversation_id, seq, sender, body, created_ms) \
             VALUES ($1, $2, $3, 'x', $4)",
        )
        .bind(&chat.id)
        .bind(seq)
        .bind(alec.id as i64)
        .bind(created)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE conversations SET last_seq = 150 WHERE id = $1")
        .bind(&chat.id)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    let capped = 100;
    assert_eq!(conversation(&cloud, &bea, &chat.id).await.unread, capped);
    let listed = cloud
        .get("/v1/conversations", bea.token())
        .await
        .ok(200)
        .json::<anyplay_api::Conversations>();
    assert_eq!(listed.conversations[0].unread, capped);
    // Under the cap the count is exact again.
    cloud
        .put(
            &format!("/v1/conversations/{}/read", chat.id),
            bea.token(),
            Some(json!({"seq": 100})),
        )
        .await
        .ok(204);
    assert_eq!(conversation(&cloud, &bea, &chat.id).await.unread, 50);
}

#[tokio::test]
async fn an_edit_may_not_add_a_direction_override() {
    let cloud = cloud().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    let sent = say(&cloud, &alec, &chat.id, "photo.png").await;
    cloud
        .patch(
            &format!("/v1/conversations/{}/messages/{}", chat.id, sent.seq),
            alec.token(),
            json!({"text": "photo\u{202E}gpj.exe"}),
        )
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn a_direct_message_is_delivered_when_the_other_device_fetches_it() {
    let cloud = cloud().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    let mut ea = cloud.events(alec.token()).await;
    let one = say(&cloud, &alec, &chat.id, "hello").await;
    let marks = conversation(&cloud, &alec, &chat.id).await.marks;
    assert!(
        marks.is_empty(),
        "nothing is delivered before it is fetched"
    );

    fetch(&cloud, &bea, &chat.id, "?after=0").await;
    let Event::Delivered { account, seq, .. } =
        ea.until(|e| matches!(e, Event::Delivered { .. })).await
    else {
        unreachable!()
    };
    assert_eq!((account.0, seq), (bea.id, one.seq));
    let marks = conversation(&cloud, &alec, &chat.id).await.marks;
    assert_eq!(
        (marks[0].delivered, marks[0].seq),
        (1, 0),
        "delivered, not read"
    );
    // A second fetch changes nothing and says nothing.
    fetch(&cloud, &bea, &chat.id, "?after=0").await;
    nothing_like(&mut ea, |e| matches!(e, Event::Delivered { .. })).await;
    // Reading takes delivered along.
    let two = say(&cloud, &alec, &chat.id, "two").await;
    cloud
        .put(
            &format!("/v1/conversations/{}/read", chat.id),
            bea.token(),
            Some(json!({"seq": two.seq})),
        )
        .await
        .ok(204);
    let marks = conversation(&cloud, &alec, &chat.id).await.marks;
    assert_eq!((marks[0].delivered, marks[0].seq), (2, 2));
}

#[tokio::test]
async fn groups_say_nothing_of_delivery() {
    let cloud = cloud().await;
    let hub = cloud.sign_up("Hub").await;
    let (bea, cy) = (cloud.sign_up("Bea").await, cloud.sign_up("Cyd").await);
    befriend(&cloud, &hub, &bea).await;
    befriend(&cloud, &hub, &cy).await;
    let group = open(&cloud, &hub, &[&bea, &cy]).await;
    let mut eh = cloud.events(hub.token()).await;
    say(&cloud, &hub, &group.id, "hi all").await;
    fetch(&cloud, &bea, &group.id, "?after=0").await;
    nothing_like(&mut eh, |e| matches!(e, Event::Delivered { .. })).await;
}

#[tokio::test]
async fn changes_are_fetched_by_revision_in_pages() {
    let cloud = cloud().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    for k in 0..4 {
        say(&cloud, &alec, &chat.id, &format!("m{k}")).await;
    }
    react(&cloud, &bea, &chat.id, 1, "gg").await.ok(200);
    edit(&cloud, &alec, &chat.id, 3, "m2!").await.ok(200);
    react(&cloud, &bea, &chat.id, 1, "wow").await.ok(200);
    unsend(&cloud, &alec, &chat.id, 4).await.ok(200);

    let page = fetch(&cloud, &bea, &chat.id, "?changed_after=0&limit=2").await;
    assert_eq!(page.rev, 4);
    assert_eq!(
        page.messages
            .iter()
            .map(|m| (m.seq, m.rev))
            .collect::<Vec<_>>(),
        [(3, 2), (1, 3)],
        "in the order of the change, a message once, at its last change"
    );
    let rest = fetch(&cloud, &bea, &chat.id, "?changed_after=3").await;
    assert_eq!(rest.messages.len(), 1);
    assert!(rest.messages[0].deleted);
    cloud
        .get(
            &format!(
                "/v1/conversations/{}/messages?changed_after=1&after=1",
                chat.id
            ),
            bea.token(),
        )
        .await
        .is_error("bad_request", 400);
}

async fn report(
    cloud: &TestCloud,
    from: &User,
    about: &User,
    conversation: &str,
    seq: u64,
) -> Reply {
    cloud
        .post(
            "/v1/reports",
            from.token(),
            json!({"account": about.id, "reason": "harassment", "details": "",
                   "conversation": conversation, "seq": seq}),
        )
        .await
}

#[tokio::test]
async fn a_report_keeps_the_text_as_reported_and_an_unsent_message_cannot_be_reported() {
    let cloud = cloud().await;
    let mona = cloud.sign_up("Mona").await;
    cloud
        .post(
            "/v1/moderation/moderators",
            ADMIN,
            json!({"handle": mona.handle}),
        )
        .await
        .ok(200);
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let cy = cloud.sign_up("Cyd").await;
    befriend(&cloud, &alec, &cy).await;
    let chat = open(&cloud, &alec, &[&bea, &cy]).await;
    let rude = say(&cloud, &bea, &chat.id, "you are the worst").await;
    let other = say(&cloud, &bea, &chat.id, "and I mean it").await;
    report(&cloud, &alec, &bea, &chat.id, rude.seq)
        .await
        .ok(204);
    // The writer takes it back and edits the other one: the report still holds the words.
    unsend(&cloud, &bea, &chat.id, rude.seq).await.ok(200);
    edit(&cloud, &bea, &chat.id, other.seq, "sorry")
        .await
        .ok(200);
    let queue: Vec<Value> = cloud
        .get("/v1/moderation/reports?status=open", mona.token())
        .await
        .ok(200)
        .json();
    assert_eq!(queue.len(), 1);
    assert!(
        queue[0]["content"]
            .as_str()
            .unwrap()
            .contains("you are the worst")
    );
    // An unsent message can still be reported while it is held, with the text it had.
    report(&cloud, &cy, &bea, &chat.id, rude.seq).await.ok(204);
    let queue: Vec<Value> = cloud
        .get("/v1/moderation/reports?status=open", mona.token())
        .await
        .ok(200)
        .json();
    assert!(queue.iter().any(|r| {
        let c = r["content"].as_str().unwrap();
        c.contains("you are the worst") && c.contains("unsent")
    }));
    // After a week the hold is gone.
    sqlx::query("UPDATE message_holds SET held_ms = held_ms - 8 * 24 * 3600 * 1000")
        .execute(&cloud.state.db)
        .await
        .unwrap();
    let held = chat_held(&cloud, &chat.id, rude.seq).await;
    assert!(!held, "a week old text is not readable any more");
}

#[tokio::test]
async fn the_export_lists_edits_unsent_messages_and_reactions() {
    let cloud = cloud().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    let one = say(&cloud, &alec, &chat.id, "first").await;
    let two = say(&cloud, &alec, &chat.id, "second").await;
    let three = say(&cloud, &bea, &chat.id, "third").await;
    edit(&cloud, &alec, &chat.id, one.seq, "first!")
        .await
        .ok(200);
    unsend(&cloud, &alec, &chat.id, two.seq).await.ok(200);
    react(&cloud, &alec, &chat.id, three.seq, "gg")
        .await
        .ok(200);
    settings(
        &cloud,
        &alec,
        &chat.id,
        json!({"muted": true, "pinned": true}),
    )
    .await
    .ok(200);

    let export: Value = cloud.get("/v1/me/export", alec.token()).await.ok(200).body;
    let written = export["messages"].as_array().unwrap();
    assert_eq!(written.len(), 2);
    assert_eq!(written[0]["text"], "first!");
    assert!(written[0]["edited"].as_u64().is_some());
    assert_eq!(written[1]["text"], "");
    assert_eq!(written[1]["unsent"], true);
    assert_eq!(export["conversations"][0]["muted"], true);
    assert_eq!(export["conversations"][0]["pinned"], true);
}

#[tokio::test]
async fn changes_have_a_limit_of_their_own() {
    let cloud = TestCloud::with(
        |c| c.new_account_hours = 0,
        |t| {
            t.rates.message_changes = 3;
            t.rates.message_changes_window = Duration::from_secs(60);
        },
    )
    .await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    let one = say(&cloud, &alec, &chat.id, "x").await;
    for _ in 0..3 {
        react(&cloud, &bea, &chat.id, one.seq, "gg").await.ok(200);
    }
    react(&cloud, &bea, &chat.id, one.seq, "wow")
        .await
        .is_error("rate_limited", 429);
    // Messages have theirs, and it is not used up by changes.
    say(&cloud, &bea, &chat.id, "still fine").await;
}

async fn chat_held(cloud: &TestCloud, conversation: &str, seq: u64) -> bool {
    let mut conn = cloud.state.db.acquire().await.unwrap();
    !anyplay_cloud::chat::held_texts(
        &mut conn,
        conversation,
        seq,
        anyplay_cloud::chat::hold_cut_ms(),
    )
    .await
    .unwrap()
    .is_empty()
}

#[tokio::test]
async fn every_replaced_text_is_held_for_a_report() {
    let cloud = cloud().await;
    let mona = cloud.sign_up("Mona").await;
    cloud
        .post(
            "/v1/moderation/moderators",
            ADMIN,
            json!({"handle": mona.handle}),
        )
        .await
        .ok(200);
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    let one = say(&cloud, &bea, &chat.id, "first words").await;
    edit(&cloud, &bea, &chat.id, one.seq, "second words")
        .await
        .ok(200);
    edit(&cloud, &bea, &chat.id, one.seq, "third words")
        .await
        .ok(200);
    report(&cloud, &alec, &bea, &chat.id, one.seq).await.ok(204);
    let queue: Vec<Value> = cloud
        .get("/v1/moderation/reports?status=open", mona.token())
        .await
        .ok(200)
        .json();
    let content = queue[0]["content"].as_str().unwrap();
    for text in ["first words", "second words", "third words"] {
        assert!(content.contains(text), "{content}");
    }
}

#[tokio::test]
async fn a_late_member_hears_nothing_of_older_messages_and_reactions_come_in_number_order() {
    let cloud = cloud().await;
    let hub = cloud.sign_up("Hub").await;
    let (bea, cyd, dee) = (
        cloud.sign_up("Bea").await,
        cloud.sign_up("Cyd").await,
        cloud.sign_up("Dee").await,
    );
    for pal in [&bea, &cyd, &dee] {
        befriend(&cloud, &hub, pal).await;
    }
    let group = open(&cloud, &hub, &[&bea, &cyd]).await;
    let old = say(&cloud, &hub, &group.id, "before dee").await;
    cloud
        .post(
            &format!("/v1/conversations/{}/members", group.id),
            hub.token(),
            json!({"accounts": [dee.id]}),
        )
        .await
        .ok(200);
    let mut late = cloud.events(dee.token()).await;
    // The later account reacts first; the answer still lists accounts by number.
    react(&cloud, &cyd, &group.id, old.seq, "gg").await.ok(200);
    let both = react(&cloud, &bea, &group.id, old.seq, "gg")
        .await
        .ok(200)
        .json::<Message>();
    let ids: Vec<u64> = both.reactions[0].accounts.iter().map(|a| a.0).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted);
    nothing_like(&mut late, |e| matches!(e, Event::MessageUpdate { .. })).await;
}

#[tokio::test]
async fn a_message_can_be_edited_only_so_often_and_the_export_lists_reactions() {
    let cloud = cloud().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let chat = open(&cloud, &alec, &[&bea]).await;
    let one = say(&cloud, &alec, &chat.id, "v0").await;
    react(&cloud, &bea, &chat.id, one.seq, "wow").await.ok(200);
    for k in 1..=20 {
        edit(&cloud, &alec, &chat.id, one.seq, &format!("v{k}"))
            .await
            .ok(200);
    }
    edit(&cloud, &alec, &chat.id, one.seq, "v21")
        .await
        .is_error("limit", 409);
    let export: Value = cloud.get("/v1/me/export", bea.token()).await.ok(200).body;
    assert_eq!(export["message_reactions"][0]["reaction"], "wow");
}

#[tokio::test]
async fn edits_unsends_and_read_marks_do_not_cross_a_block_in_a_group() {
    let cloud = cloud().await;
    let hub = cloud.sign_up("Hub").await;
    let (bea, cyd) = (cloud.sign_up("Bea").await, cloud.sign_up("Cyd").await);
    for pal in [&bea, &cyd] {
        befriend(&cloud, &hub, pal).await;
    }
    let group = open(&cloud, &hub, &[&bea, &cyd]).await;
    let one = say(&cloud, &hub, &group.id, "first").await;
    let two = say(&cloud, &hub, &group.id, "second").await;
    cloud
        .put(&format!("/v1/blocks/{}", hub.id), cyd.token(), None)
        .await
        .ok(204);
    let mut eb = cloud.events(bea.token()).await;
    let mut ec = cloud.events(cyd.token()).await;
    let mut eh = cloud.events(hub.token()).await;

    edit(&cloud, &hub, &group.id, one.seq, "first!")
        .await
        .ok(200);
    unsend(&cloud, &hub, &group.id, two.seq).await.ok(200);
    eb.until(|e| matches!(e, Event::MessageUpdate { .. })).await;
    nothing_like(&mut ec, |e| matches!(e, Event::MessageUpdate { .. })).await;

    // Reading is not told across the block either way.
    cloud
        .put(
            &format!("/v1/conversations/{}/read", group.id),
            cyd.token(),
            Some(json!({"seq": 2})),
        )
        .await
        .ok(204);
    nothing_like(&mut eh, |e| matches!(e, Event::Read { .. })).await;
    // ...and its mark is not in the conversation the blocked account reads.
    let seen = conversation(&cloud, &hub, &group.id).await;
    assert!(seen.marks.iter().all(|m| m.account.0 != cyd.id));
}

#[tokio::test]
async fn a_conversation_with_too_many_accounts_is_refused_at_once() {
    let cloud = cloud().await;
    let hub = cloud.sign_up("Hub").await;
    let accounts: Vec<u64> = (1..=5000).collect();
    cloud
        .post(
            "/v1/conversations",
            hub.token(),
            json!({"accounts": accounts}),
        )
        .await
        .is_error("bad_request", 400);
}
