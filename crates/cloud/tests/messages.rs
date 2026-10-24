mod common;

use std::sync::Arc;
use std::time::Duration;

use anyplay_api::{
    Added, Conversation, ConversationKind, Conversations, Event, MAX_GROUP_MEMBERS,
    MAX_MESSAGE_CHARS, Message, Messages,
};
use common::{Events, Reply, TestCloud, User};
use serde_json::{Value, json};

const QUIET: Duration = Duration::from_millis(300);
const DAY: i64 = 24 * 60 * 60 * 1000;

/// A cloud where new accounts may write to strangers at once.
async fn open_cloud() -> TestCloud {
    TestCloud::with(|c| c.new_account_hours = 0, |_| {}).await
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

/// A user with `n` friends, none of them friends with each other.
async fn hub_and_friends(cloud: &TestCloud, n: usize) -> (User, Vec<User>) {
    let hub = cloud.sign_up("Hub").await;
    let mut friends = Vec::new();
    for k in 0..n {
        let friend = cloud.sign_up(&format!("Pal{k}")).await;
        befriend(cloud, &hub, &friend).await;
        friends.push(friend);
    }
    (hub, friends)
}

async fn open(cloud: &TestCloud, from: &User, with: &[&User]) -> Reply {
    cloud
        .post(
            "/v1/conversations",
            from.token(),
            json!({"accounts": with.iter().map(|u| u.id).collect::<Vec<_>>()}),
        )
        .await
}

async fn open_one(cloud: &TestCloud, from: &User, with: &User) -> Conversation {
    open(cloud, from, &[with]).await.ok(200).json()
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

async fn texts(cloud: &TestCloud, user: &User, conversation: &str) -> Vec<String> {
    fetch(cloud, user, conversation, "?limit=100")
        .await
        .messages
        .into_iter()
        .map(|m| m.text)
        .collect()
}

async fn lists(cloud: &TestCloud, user: &User) -> Conversations {
    cloud
        .get("/v1/conversations", user.token())
        .await
        .ok(200)
        .json()
}

async fn pointer(events: &mut Events) -> (String, u64) {
    match events.until(|e| matches!(e, Event::Message { .. })).await {
        Event::Message { conversation, seq } => (conversation, seq),
        _ => unreachable!(),
    }
}

async fn no_message(events: &mut Events) {
    let deadline = tokio::time::Instant::now() + QUIET;
    while let Some(event) = events
        .next_within(deadline.saturating_duration_since(tokio::time::Instant::now()))
        .await
    {
        assert!(
            !matches!(
                event,
                Event::Message { .. } | Event::MessageRequest { .. } | Event::Read { .. }
            ),
            "unexpected {event:?}"
        );
    }
}

#[tokio::test]
async fn friends_chat_in_numbered_order_and_hear_pointers() {
    let cloud = TestCloud::start().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let mut ea = cloud.events(alec.token()).await;
    let mut eb = cloud.events(bea.token()).await;

    let conversation = open_one(&cloud, &alec, &bea).await;
    assert_eq!(conversation.kind, ConversationKind::Direct);
    assert_eq!(conversation.members.len(), 2);
    assert_eq!((conversation.last, conversation.read), (0, 0));
    assert!(!conversation.request);
    // Opening it again finds the same one, from either side.
    assert_eq!(open_one(&cloud, &bea, &alec).await.id, conversation.id);
    assert_eq!(open_one(&cloud, &alec, &bea).await.id, conversation.id);

    let one: Message = say(&cloud, &alec, &conversation.id, "one")
        .await
        .ok(200)
        .json();
    let two: Message = say(&cloud, &alec, &conversation.id, "two")
        .await
        .ok(200)
        .json();
    assert_eq!((one.seq, two.seq), (1, 2));
    assert_eq!(one.sender.handle, "Alec");
    assert!(one.created > 1_700_000_000);

    // The receiver hears a pointer for each number, and so do the sender's other devices.
    assert_eq!(pointer(&mut eb).await, (conversation.id.clone(), 1));
    assert_eq!(pointer(&mut eb).await, (conversation.id.clone(), 2));
    assert_eq!(pointer(&mut ea).await, (conversation.id.clone(), 1));

    say(&cloud, &bea, &conversation.id, "three").await.ok(200);
    let page = fetch(&cloud, &bea, &conversation.id, "?after=0").await;
    assert_eq!(page.last, 3);
    assert_eq!(
        page.messages.iter().map(|m| m.seq).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(page.messages[2].sender.handle, "Bea");
    let later = fetch(&cloud, &alec, &conversation.id, "?after=1").await;
    assert_eq!(later.messages.len(), 2);
    let earlier = fetch(&cloud, &alec, &conversation.id, "?before=3").await;
    assert_eq!(
        earlier
            .messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>(),
        ["one", "two"]
    );
    let newest = fetch(&cloud, &alec, &conversation.id, "?limit=2").await;
    assert_eq!(
        newest
            .messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>(),
        ["two", "three"]
    );
    cloud
        .get(
            &format!(
                "/v1/conversations/{}/messages?after=1&before=3",
                conversation.id
            ),
            alec.token(),
        )
        .await
        .is_error("bad_request", 400);

    // Both lists show it, newest first, with the numbers a client compares.
    let listed = lists(&cloud, &bea).await;
    assert!(listed.requests.is_empty());
    assert_eq!(listed.conversations.len(), 1);
    assert_eq!(listed.conversations[0].last, 3);
    assert_eq!(listed.conversations[0].read, 3, "writing reads to there");
}

#[tokio::test]
async fn read_marks_move_forward_and_are_told_to_the_other_side() {
    let cloud = TestCloud::start().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let conversation = open_one(&cloud, &alec, &bea).await;
    for text in ["a", "b", "c"] {
        say(&cloud, &alec, &conversation.id, text).await.ok(200);
    }
    let mut ea = cloud.events(alec.token()).await;
    let mut eb = cloud.events(bea.token()).await;

    let mark = |seq: u64| json!({"seq": seq});
    let path = format!("/v1/conversations/{}/read", conversation.id);
    cloud.put(&path, bea.token(), Some(mark(2))).await.ok(204);
    for events in [&mut ea, &mut eb] {
        match events.until(|e| matches!(e, Event::Read { .. })).await {
            Event::Read {
                conversation: id,
                account,
                seq,
            } => assert_eq!((id, account.0, seq), (conversation.id.clone(), bea.id, 2)),
            _ => unreachable!(),
        }
    }
    // Reading less, or the same again, changes nothing and tells nobody.
    cloud.put(&path, bea.token(), Some(mark(1))).await.ok(204);
    cloud.put(&path, bea.token(), Some(mark(2))).await.ok(204);
    no_message(&mut ea).await;
    // Beyond the newest message counts as reading all of them.
    cloud.put(&path, bea.token(), Some(mark(99))).await.ok(204);
    match ea.until(|e| matches!(e, Event::Read { .. })).await {
        Event::Read { seq, .. } => assert_eq!(seq, 3),
        _ => unreachable!(),
    }

    let seen = lists(&cloud, &alec).await.conversations.remove(0);
    assert_eq!(seen.marks.len(), 1);
    assert_eq!((seen.marks[0].account.0, seen.marks[0].seq), (bea.id, 3));
    let own = lists(&cloud, &bea).await.conversations.remove(0);
    assert_eq!((own.read, own.last), (3, 3));
    // Outsiders have no conversation to read.
    let outsider = cloud.sign_up("Outsider").await;
    cloud
        .put(&path, outsider.token(), Some(mark(1)))
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn message_text_is_bounded() {
    let cloud = TestCloud::start().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let conversation = open_one(&cloud, &alec, &bea).await;
    let id = &conversation.id;
    say(&cloud, &alec, id, &"ä".repeat(MAX_MESSAGE_CHARS))
        .await
        .ok(200);
    for bad in [
        "".to_owned(),
        " \n\t ".to_owned(),
        "ä".repeat(MAX_MESSAGE_CHARS + 1),
        "bell\u{7}".to_owned(),
    ] {
        say(&cloud, &alec, id, &bad)
            .await
            .is_error("bad_request", 400);
    }
    // Line breaks are fine, and what was stored comes back as it was written.
    say(&cloud, &alec, id, "two\nlines").await.ok(200);
    assert_eq!(texts(&cloud, &bea, id).await[1], "two\nlines");
    cloud
        .post(
            &format!("/v1/conversations/{id}/messages"),
            alec.token(),
            json!({"nothing": true}),
        )
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn a_stranger_writes_into_a_request_that_waits_to_be_accepted() {
    let cloud = open_cloud().await;
    let alec = cloud.sign_up("Alec").await;
    let sam = cloud.sign_up("Sam").await;
    let mut es = cloud.events(sam.token()).await;

    let conversation = open_one(&cloud, &alec, &sam).await;
    // An opened conversation without a message is no request yet.
    assert!(lists(&cloud, &sam).await.requests.is_empty());
    say(&cloud, &alec, &conversation.id, "hello stranger")
        .await
        .ok(200);
    match es
        .until(|e| matches!(e, Event::MessageRequest { .. }))
        .await
    {
        Event::MessageRequest { conversation: id } => assert_eq!(id, conversation.id),
        _ => unreachable!(),
    }
    // Told once; more messages wait in the request.
    say(&cloud, &alec, &conversation.id, "are you there")
        .await
        .ok(200);
    no_message(&mut es).await;

    let sams = lists(&cloud, &sam).await;
    assert!(sams.conversations.is_empty());
    assert_eq!(sams.requests.len(), 1);
    assert!(sams.requests[0].request);
    assert_eq!(sams.requests[0].last, 2);
    // Alec sees an ordinary conversation, and Sam can read the request to decide.
    let alecs = lists(&cloud, &alec).await;
    assert_eq!(alecs.conversations.len(), 1);
    assert!(!alecs.conversations[0].request);
    assert_eq!(
        texts(&cloud, &sam, &conversation.id).await,
        ["hello stranger", "are you there"]
    );
    // What Sam reads of a request is not told to the stranger.
    let mut ea = cloud.events(alec.token()).await;
    let path = format!("/v1/conversations/{}/read", conversation.id);
    cloud
        .put(&path, sam.token(), Some(json!({"seq": 2})))
        .await
        .ok(204);
    no_message(&mut ea).await;
    assert!(lists(&cloud, &alec).await.conversations[0].marks.is_empty());

    // Accepted, it is a conversation, and then reads and messages flow both ways.
    let accepted: Conversation = cloud
        .post_empty(
            &format!("/v1/conversations/{}/accept", conversation.id),
            sam.token(),
        )
        .await
        .ok(200)
        .json();
    assert!(!accepted.request);
    assert!(lists(&cloud, &sam).await.requests.is_empty());
    say(&cloud, &alec, &conversation.id, "good").await.ok(200);
    assert_eq!(pointer(&mut es).await, (conversation.id.clone(), 3));
    cloud
        .put(&path, sam.token(), Some(json!({"seq": 3})))
        .await
        .ok(204);
    match ea.until(|e| matches!(e, Event::Read { .. })).await {
        Event::Read { account, seq, .. } => assert_eq!((account.0, seq), (sam.id, 3)),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn declining_a_request_hides_it_and_the_sender_is_not_told() {
    let cloud = open_cloud().await;
    let alec = cloud.sign_up("Alec").await;
    let sam = cloud.sign_up("Sam").await;
    let mut es = cloud.events(sam.token()).await;
    let conversation = open_one(&cloud, &alec, &sam).await;
    say(&cloud, &alec, &conversation.id, "psst").await.ok(200);
    cloud
        .delete(
            &format!("/v1/conversations/{}", conversation.id),
            sam.token(),
        )
        .await
        .ok(204);
    assert!(lists(&cloud, &sam).await.requests.is_empty());
    cloud
        .get(
            &format!("/v1/conversations/{}", conversation.id),
            sam.token(),
        )
        .await
        .is_error("not_found", 404);

    // Alec writes on: it is accepted, stored, and goes nowhere.
    say(&cloud, &alec, &conversation.id, "hello?").await.ok(200);
    let _ = es
        .until(|e| matches!(e, Event::MessageRequest { .. }))
        .await;
    no_message(&mut es).await;
    assert!(lists(&cloud, &sam).await.requests.is_empty());

    // Sam can still reach out, which opens it again, with a view from this moment on.
    let reopened = open_one(&cloud, &sam, &alec).await;
    assert_eq!(reopened.id, conversation.id);
    assert!(!reopened.request);
    assert!(texts(&cloud, &sam, &conversation.id).await.is_empty());
    say(&cloud, &alec, &conversation.id, "there you are")
        .await
        .ok(200);
    assert_eq!(
        texts(&cloud, &sam, &conversation.id).await,
        ["there you are"]
    );
}

#[tokio::test]
async fn answering_a_request_accepts_it_and_a_friendship_turns_it_into_a_chat() {
    let cloud = open_cloud().await;
    let alec = cloud.sign_up("Alec").await;
    let sam = cloud.sign_up("Sam").await;
    let bea = cloud.sign_up("Bea").await;

    let to_sam = open_one(&cloud, &alec, &sam).await;
    say(&cloud, &alec, &to_sam.id, "hi").await.ok(200);
    say(&cloud, &sam, &to_sam.id, "hi back").await.ok(200);
    let sams = lists(&cloud, &sam).await;
    assert!(sams.requests.is_empty(), "answering accepted it");
    assert_eq!(sams.conversations.len(), 1);

    let to_bea = open_one(&cloud, &alec, &bea).await;
    say(&cloud, &alec, &to_bea.id, "hello").await.ok(200);
    assert_eq!(lists(&cloud, &bea).await.requests.len(), 1);
    befriend(&cloud, &alec, &bea).await;
    let beas = lists(&cloud, &bea).await;
    assert!(beas.requests.is_empty());
    assert_eq!(beas.conversations.len(), 1);
    // A request from a friend cannot be declined away.
    cloud
        .delete(&format!("/v1/conversations/{}", to_bea.id), bea.token())
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn a_stranger_cannot_pile_up_messages_before_an_answer() {
    let cloud = open_cloud().await;
    let alec = cloud.sign_up("Alec").await;
    let sam = cloud.sign_up("Sam").await;
    let conversation = open_one(&cloud, &alec, &sam).await;
    for n in 0..5 {
        say(&cloud, &alec, &conversation.id, &format!("m{n}"))
            .await
            .ok(200);
    }
    say(&cloud, &alec, &conversation.id, "m5")
        .await
        .is_error("limit", 409);
    // Sam's answer lifts it.
    say(&cloud, &sam, &conversation.id, "ok").await.ok(200);
    say(&cloud, &alec, &conversation.id, "m5").await.ok(200);
}

#[tokio::test]
async fn a_new_account_may_not_write_to_strangers_but_may_write_to_friends() {
    let cloud = TestCloud::with(|c| c.new_account_hours = 24, |_| {}).await;
    let alec = cloud.sign_up("Alec").await;
    let sam = cloud.sign_up("Sam").await;
    open(&cloud, &alec, &[&sam])
        .await
        .is_error("restricted", 403);
    // Friends are fine at once.
    let bea = cloud.sign_up("Bea").await;
    befriend(&cloud, &alec, &bea).await;
    open_one(&cloud, &alec, &bea).await;

    // A day later the same account may.
    sqlx::query("UPDATE accounts SET created = created - 172800 WHERE id = $1")
        .bind(alec.id as i64)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    open_one(&cloud, &alec, &sam).await;
}

#[tokio::test]
async fn stranger_conversations_are_rate_limited() {
    let cloud = TestCloud::with(|c| c.new_account_hours = 0, |t| t.rates.strangers = 2).await;
    let alec = cloud.sign_up("Alec").await;
    for n in 0..2 {
        let other = cloud.sign_up(&format!("Other{n}")).await;
        open_one(&cloud, &alec, &other).await;
    }
    let third = cloud.sign_up("Third").await;
    let reply = open(&cloud, &alec, &[&third]).await;
    reply.is_error("rate_limited", 429);
    assert!(reply.headers.contains_key("retry-after"));
}

#[tokio::test]
async fn blocks_stop_delivery_both_ways() {
    let cloud = TestCloud::start().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let conversation = open_one(&cloud, &alec, &bea).await;
    let mut eb = cloud.events(bea.token()).await;
    say(&cloud, &alec, &conversation.id, "before").await.ok(200);
    assert_eq!(pointer(&mut eb).await.1, 1);

    // Bea blocks Alec. Alec can still write, and nothing reaches Bea, then or later.
    cloud
        .put(&format!("/v1/blocks/{}", alec.id), bea.token(), None)
        .await
        .ok(204);
    say(&cloud, &alec, &conversation.id, "while blocked")
        .await
        .ok(200);
    no_message(&mut eb).await;
    assert_eq!(texts(&cloud, &bea, &conversation.id).await, ["before"]);
    cloud
        .delete(&format!("/v1/blocks/{}", alec.id), bea.token())
        .await
        .ok(204);
    say(&cloud, &alec, &conversation.id, "after").await.ok(200);
    assert_eq!(pointer(&mut eb).await.1, 3);
    assert_eq!(
        texts(&cloud, &bea, &conversation.id).await,
        ["before", "after"],
        "what was kept away stays away"
    );
    assert_eq!(
        texts(&cloud, &alec, &conversation.id).await,
        ["before", "while blocked", "after"]
    );

    // Alec blocks Bea: writing and opening are refused until he lifts it.
    cloud
        .put(&format!("/v1/blocks/{}", bea.id), alec.token(), None)
        .await
        .ok(204);
    say(&cloud, &alec, &conversation.id, "no")
        .await
        .is_error("forbidden", 403);
    open(&cloud, &alec, &[&bea])
        .await
        .is_error("forbidden", 403);
    // And what Bea writes does not reach Alec.
    let mut ea = cloud.events(alec.token()).await;
    say(&cloud, &bea, &conversation.id, "hello?").await.ok(200);
    no_message(&mut ea).await;
    assert_eq!(texts(&cloud, &alec, &conversation.id).await.len(), 3);
}

#[tokio::test]
async fn in_a_group_a_block_hides_one_senders_messages_from_one_member() {
    let cloud = TestCloud::start().await;
    let (hub, friends) = hub_and_friends(&cloud, 2).await;
    let (bea, cy) = (&friends[0], &friends[1]);
    let group: Conversation = open(&cloud, &hub, &[bea, cy]).await.ok(200).json();
    let mut ec = cloud.events(cy.token()).await;
    cloud
        .put(&format!("/v1/blocks/{}", bea.id), cy.token(), None)
        .await
        .ok(204);
    say(&cloud, bea, &group.id, "from Bea").await.ok(200);
    say(&cloud, &hub, &group.id, "from Hub").await.ok(200);
    assert_eq!(texts(&cloud, cy, &group.id).await, ["from Hub"]);
    assert_eq!(
        texts(&cloud, &hub, &group.id).await,
        ["from Bea", "from Hub"]
    );
    // Cy is told of the one it may see.
    assert_eq!(pointer(&mut ec).await.1, 2);
    // Nobody can add a member that a member blocked.
    let dee = cloud.sign_up("Dee").await;
    befriend(&cloud, &hub, &dee).await;
    cloud
        .put(&format!("/v1/blocks/{}", dee.id), cy.token(), None)
        .await
        .ok(204);
    let added: Added = cloud
        .post(
            &format!("/v1/conversations/{}/members", group.id),
            hub.token(),
            json!({"accounts": [dee.id]}),
        )
        .await
        .ok(200)
        .json();
    assert!(added.added.is_empty());
}

#[tokio::test]
async fn a_group_is_made_of_friends_and_members_come_and_go() {
    let cloud = TestCloud::start().await;
    let (hub, friends) = hub_and_friends(&cloud, 3).await;
    let (bea, cy, dee) = (&friends[0], &friends[1], &friends[2]);
    let mut eb = cloud.events(bea.token()).await;
    let mut ec = cloud.events(cy.token()).await;
    let mut ed = cloud.events(dee.token()).await;

    // Only friends of the creator can be named, and not oneself.
    let stranger = cloud.sign_up("Stranger").await;
    open(&cloud, &hub, &[bea, &stranger])
        .await
        .is_error("forbidden", 403);
    open(&cloud, &hub, &[&hub])
        .await
        .is_error("bad_request", 400);
    open(&cloud, &hub, &[]).await.is_error("bad_request", 400);

    let group: Conversation = cloud
        .post(
            "/v1/conversations",
            hub.token(),
            json!({"accounts": [bea.id, cy.id], "name": "Raid night"}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(group.kind, ConversationKind::Group);
    assert_eq!(group.name.as_deref(), Some("Raid night"));
    assert_eq!(group.members.len(), 3);
    for events in [&mut eb, &mut ec] {
        match events
            .until(|e| matches!(e, Event::Conversation { .. }))
            .await
        {
            Event::Conversation { id } => assert_eq!(id, group.id),
            _ => unreachable!(),
        }
    }

    say(&cloud, bea, &group.id, "first").await.ok(200);
    assert_eq!(pointer(&mut ec).await.1, 1);
    // Strangers to each other, Bea and Cy are one group: no request, they see each other.
    assert!(lists(&cloud, cy).await.requests.is_empty());
    assert_eq!(texts(&cloud, cy, &group.id).await, ["first"]);

    // A member adds another of its friends; Dee sees only what comes after.
    let added: Added = cloud
        .post(
            &format!("/v1/conversations/{}/members", group.id),
            hub.token(),
            json!({"accounts": [dee.id, stranger.id, bea.id]}),
        )
        .await
        .ok(200)
        .json();
    assert_eq!(
        added.added.len(),
        1,
        "Dee only: Stranger is no friend, Bea is in"
    );
    match ed.until(|e| matches!(e, Event::Conversation { .. })).await {
        Event::Conversation { id } => assert_eq!(id, group.id),
        _ => unreachable!(),
    }
    say(&cloud, &hub, &group.id, "welcome Dee").await.ok(200);
    assert_eq!(texts(&cloud, dee, &group.id).await, ["welcome Dee"]);
    // Members that are not friends of the adder are not added by it.
    cloud
        .post(
            &format!("/v1/conversations/{}/members", group.id),
            bea.token(),
            json!({"accounts": [stranger.id]}),
        )
        .await
        .ok(200);
    assert_eq!(lists(&cloud, &hub).await.conversations[0].members.len(), 4);

    // Leaving: the leaver is told it is gone, the rest that it changed.
    cloud
        .delete(&format!("/v1/conversations/{}", group.id), cy.token())
        .await
        .ok(204);
    assert!(matches!(
        ec.until(|e| matches!(e, Event::ConversationGone { .. })).await,
        Event::ConversationGone { id } if id == group.id
    ));
    assert!(matches!(
        eb.until(|e| matches!(e, Event::Conversation { .. })).await,
        Event::Conversation { .. }
    ));
    say(&cloud, cy, &group.id, "still here?")
        .await
        .is_error("not_found", 404);
    assert_eq!(lists(&cloud, &hub).await.conversations[0].members.len(), 3);

    // The last to leave ends it.
    for user in [bea, dee, &hub] {
        cloud
            .delete(&format!("/v1/conversations/{}", group.id), user.token())
            .await
            .ok(204);
    }
    cloud
        .get(&format!("/v1/conversations/{}", group.id), hub.token())
        .await
        .is_error("not_found", 404);
}

#[tokio::test]
async fn a_group_holds_thirty_two_and_a_friendship_conversation_cannot_be_left() {
    let cloud = TestCloud::start().await;
    let (hub, friends) = hub_and_friends(&cloud, 32).await;
    let first: Vec<&User> = friends[..31].iter().collect();
    let group: Conversation = open(&cloud, &hub, &first).await.ok(200).json();
    assert_eq!(group.members.len(), usize::from(MAX_GROUP_MEMBERS));
    // One more does not fit.
    cloud
        .post(
            &format!("/v1/conversations/{}/members", group.id),
            hub.token(),
            json!({"accounts": [friends[31].id]}),
        )
        .await
        .is_error("full", 409);
    // Too many to start with.
    let all: Vec<&User> = friends.iter().collect();
    open(&cloud, &hub, &all).await.is_error("bad_request", 400);

    let direct = open_one(&cloud, &hub, &friends[0]).await;
    cloud
        .delete(&format!("/v1/conversations/{}", direct.id), hub.token())
        .await
        .is_error("bad_request", 400);
}

#[tokio::test]
async fn a_client_that_was_away_recovers_by_number() {
    let cloud = TestCloud::start().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let conversation = open_one(&cloud, &alec, &bea).await;
    // Bea reads the first two while she is online, then goes away.
    for n in 1..=2 {
        say(&cloud, &alec, &conversation.id, &format!("m{n}"))
            .await
            .ok(200);
    }
    let known = fetch(&cloud, &bea, &conversation.id, "?after=0").await.last;
    assert_eq!(known, 2);
    // Five more are written while no event connection of hers exists.
    for n in 3..=7 {
        say(&cloud, &alec, &conversation.id, &format!("m{n}"))
            .await
            .ok(200);
    }
    // She comes back: welcome first, then the list tells her what changed.
    let mut eb = cloud.events(bea.token()).await;
    let list = lists(&cloud, &bea).await;
    let item = &list.conversations[0];
    assert_eq!((item.last, item.read), (7, 0));
    assert!(item.last > known);
    // She fetches in pages from the number she holds and misses nothing.
    let mut have = known;
    let mut got = Vec::new();
    while have < item.last {
        let page = fetch(
            &cloud,
            &bea,
            &conversation.id,
            &format!("?after={have}&limit=2"),
        )
        .await;
        assert!(!page.messages.is_empty());
        for message in page.messages {
            assert_eq!(message.seq, have + 1, "no gap, no repeat");
            have = message.seq;
            got.push(message.text);
        }
    }
    assert_eq!(got, ["m3", "m4", "m5", "m6", "m7"]);
    // And a message that arrives later is announced as usual.
    say(&cloud, &alec, &conversation.id, "m8").await.ok(200);
    assert_eq!(pointer(&mut eb).await.1, 8);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writers_at_the_same_moment_never_get_the_same_number() {
    let cloud = Arc::new(TestCloud::start().await);
    let (hub, friends) = hub_and_friends(&cloud, 3).await;
    let refs: Vec<&User> = friends.iter().collect();
    let group: Conversation = open(&cloud, &hub, &refs).await.ok(200).json();

    let mut writers = Vec::new();
    let senders: Vec<(String, String)> = std::iter::once(&hub)
        .chain(friends.iter())
        .map(|u| (u.token().to_owned(), u.handle.clone()))
        .collect();
    for (token, _) in &senders {
        for n in 0..10 {
            let cloud = cloud.clone();
            let token = token.clone();
            let id = group.id.clone();
            writers.push(tokio::spawn(async move {
                let reply = cloud
                    .post(
                        &format!("/v1/conversations/{id}/messages"),
                        &token,
                        json!({"text": format!("m{n}")}),
                    )
                    .await
                    .ok(200);
                reply.json::<Message>().seq
            }));
        }
    }
    let mut numbers = Vec::new();
    for writer in writers {
        numbers.push(writer.await.unwrap());
    }
    numbers.sort_unstable();
    assert_eq!(numbers, (1..=40).collect::<Vec<u64>>());

    // What is stored matches what the writers were told, number by number.
    let stored = fetch(&cloud, &hub, &group.id, "?limit=100").await;
    assert_eq!(stored.last, 40);
    assert_eq!(
        stored.messages.iter().map(|m| m.seq).collect::<Vec<_>>(),
        (1..=40).collect::<Vec<u64>>()
    );
}

#[tokio::test]
async fn old_messages_and_empty_conversations_are_swept_away() {
    let cloud = TestCloud::with(
        |c| {
            c.new_account_hours = 0;
            c.message_retention_days = 30;
        },
        |_| {},
    )
    .await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let conversation = open_one(&cloud, &alec, &bea).await;
    for text in ["old", "older", "new"] {
        say(&cloud, &alec, &conversation.id, text).await.ok(200);
    }
    let cam = cloud.sign_up("Cam").await;
    let empty = open_one(&cloud, &alec, &cam).await;
    // A message that a block kept from Bea goes with the message.
    cloud
        .put(&format!("/v1/blocks/{}", alec.id), bea.token(), None)
        .await
        .ok(204);
    say(&cloud, &alec, &conversation.id, "hidden and old")
        .await
        .ok(200);

    let long_ago = util_now_ms() - 31 * DAY;
    sqlx::query("UPDATE messages SET created_ms = $1 WHERE seq <> 3")
        .bind(long_ago)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE conversations SET updated_ms = $1 WHERE id = $2")
        .bind(long_ago)
        .bind(&empty.id)
        .execute(&cloud.state.db)
        .await
        .unwrap();

    wait_until(|| async { texts(&cloud, &alec, &conversation.id).await == ["new"] }).await;
    // The numbers go on where they were.
    let next: Message = say(&cloud, &alec, &conversation.id, "later")
        .await
        .ok(200)
        .json();
    assert_eq!(next.seq, 5);
    let (hidden,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM message_hidden")
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(
        hidden, 1,
        "only the record of the message written after the sweep is left"
    );
    // The conversation with nothing in it is gone too; the one with messages stays.
    cloud
        .get(&format!("/v1/conversations/{}", empty.id), alec.token())
        .await
        .is_error("not_found", 404);
    cloud
        .get(
            &format!("/v1/conversations/{}", conversation.id),
            alec.token(),
        )
        .await
        .ok(200);
}

#[tokio::test]
async fn how_long_messages_are_kept_is_a_setting() {
    let cloud = TestCloud::with(|c| c.message_retention_days = 1, |_| {}).await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let conversation = open_one(&cloud, &alec, &bea).await;
    say(&cloud, &alec, &conversation.id, "two days old")
        .await
        .ok(200);
    say(&cloud, &alec, &conversation.id, "fresh").await.ok(200);
    sqlx::query("UPDATE messages SET created_ms = $1 WHERE seq = 1")
        .bind(util_now_ms() - 2 * DAY)
        .execute(&cloud.state.db)
        .await
        .unwrap();
    wait_until(|| async { texts(&cloud, &bea, &conversation.id).await == ["fresh"] }).await;
}

#[tokio::test]
async fn the_export_holds_the_accounts_own_messages_and_deleting_removes_them() {
    let cloud = TestCloud::start().await;
    let (hub, friends) = hub_and_friends(&cloud, 2).await;
    let (bea, cy) = (&friends[0], &friends[1]);
    let direct = open_one(&cloud, &hub, bea).await;
    let group: Conversation = open(&cloud, &hub, &[bea, cy]).await.ok(200).json();
    say(&cloud, &hub, &direct.id, "hub to bea").await.ok(200);
    say(&cloud, bea, &direct.id, "bea to hub").await.ok(200);
    say(&cloud, &hub, &group.id, "hub to all").await.ok(200);
    say(&cloud, cy, &group.id, "cy to all").await.ok(200);

    let export: Value = cloud.get("/v1/me/export", hub.token()).await.ok(200).body;
    let mut written: Vec<&str> = export["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap())
        .collect();
    written.sort_unstable();
    assert_eq!(written, ["hub to all", "hub to bea"], "own messages only");
    assert_eq!(export["conversations"].as_array().unwrap().len(), 2);
    assert!(export["party"].is_null());

    // Deleting the account deletes its messages: the direct conversation is gone for the
    // other side and the group goes on without them.
    let mut eb = cloud.events(bea.token()).await;
    let mut ec = cloud.events(cy.token()).await;
    cloud.delete("/v1/me", hub.token()).await.ok(204);
    assert!(matches!(
        eb.until(|e| matches!(e, Event::ConversationGone { .. })).await,
        Event::ConversationGone { id } if id == direct.id
    ));
    assert!(matches!(
        ec.until(|e| matches!(e, Event::Conversation { .. })).await,
        Event::Conversation { id } if id == group.id
    ));
    cloud
        .get(&format!("/v1/conversations/{}", direct.id), bea.token())
        .await
        .is_error("not_found", 404);
    assert_eq!(texts(&cloud, cy, &group.id).await, ["cy to all"]);
    let (left,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages WHERE sender = $1")
        .bind(hub.id as i64)
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn messages_are_rate_limited_per_account() {
    let cloud = TestCloud::with(|_| {}, |t| t.rates.messages = 3).await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let conversation = open_one(&cloud, &alec, &bea).await;
    for n in 0..3 {
        say(&cloud, &alec, &conversation.id, &format!("m{n}"))
            .await
            .ok(200);
    }
    let reply = say(&cloud, &alec, &conversation.id, "m3").await;
    reply.is_error("rate_limited", 429);
    assert!(reply.headers.contains_key("retry-after"));
    // Another account has its own allowance.
    say(&cloud, &bea, &conversation.id, "mine").await.ok(200);
    assert_eq!(texts(&cloud, &bea, &conversation.id).await.len(), 4);
}

#[tokio::test]
async fn nobody_outside_a_conversation_reads_or_writes_it() {
    let cloud = TestCloud::start().await;
    let (alec, bea) = cloud.friends("Alec", "Bea").await;
    let conversation = open_one(&cloud, &alec, &bea).await;
    say(&cloud, &alec, &conversation.id, "private")
        .await
        .ok(200);
    let eve = cloud.sign_up("Eve").await;
    let id = &conversation.id;
    cloud
        .get(&format!("/v1/conversations/{id}"), eve.token())
        .await
        .is_error("not_found", 404);
    cloud
        .get(&format!("/v1/conversations/{id}/messages"), eve.token())
        .await
        .is_error("not_found", 404);
    say(&cloud, &eve, id, "hi").await.is_error("not_found", 404);
    cloud
        .post_empty(&format!("/v1/conversations/{id}/accept"), eve.token())
        .await
        .is_error("not_found", 404);
    cloud
        .post(
            &format!("/v1/conversations/{id}/members"),
            eve.token(),
            json!({"accounts": [alec.id]}),
        )
        .await
        .is_error("not_found", 404);
    // A made-up conversation is no different.
    say(&cloud, &eve, "no-such-thing", "hi")
        .await
        .is_error("not_found", 404);
}

fn util_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Waits a few seconds for a sweep to do its work.
async fn wait_until<F, Fut>(check: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..80 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the sweep did not do its work in time");
}
