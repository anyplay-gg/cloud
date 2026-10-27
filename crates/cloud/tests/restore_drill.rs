//! The restore drill, driven by `.github/workflows/restore-drill.yml` (and runnable by hand).
//!
//! Without `ANYPLAY_DRILL` both tests return at once, so a normal `cargo test` ignores them.
//!
//! * `ANYPLAY_DRILL=seed`: a cloud runs on `ANYPLAY_DRILL_DATA` (a fresh folder), three players
//!   sign up, two become friends and form a party. While the cloud is still running and open,
//!   `deploy/snapshot.py` takes the online snapshot into `ANYPLAY_DRILL_ARCHIVE`. Then one more
//!   account signs up, which the snapshot must not contain. What the verify phase needs is written
//!   to `ANYPLAY_DRILL_STATE`.
//! * `ANYPLAY_DRILL=verify`: the archive was unpacked into a clean folder (`ANYPLAY_DRILL_DATA`)
//!   and a new cloud starts on it. The old access token still works (the signing key came back),
//!   a player signs in by e-mail code, the friendship and the party are there, and the account
//!   made after the snapshot is not.

mod common;

use std::process::Command;

use common::TestCloud;
use serde_json::{Value, json};

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set for the drill"))
}

fn phase() -> Option<String> {
    std::env::var("ANYPLAY_DRILL")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

#[tokio::test]
async fn drill_seed() {
    if phase().as_deref() != Some("seed") {
        return;
    }
    let data = var("ANYPLAY_DRILL_DATA");
    let cloud = TestCloud::with(move |c| c.data_dir = data.into(), |_| {}).await;
    let (alec, bea) = cloud.friends("DrillAlec", "DrillBea").await;
    let cy = cloud.sign_up("DrillCy").await;

    // A party of alec and bea.
    cloud
        .post(
            "/v1/party/invitations",
            alec.token(),
            json!({"to": [bea.id]}),
        )
        .await
        .ok(200);
    let invitations = cloud
        .get("/v1/party/invitations", bea.token())
        .await
        .ok(200);
    let invitation = invitations.body["invitations"][0]["id"]
        .as_str()
        .expect("bea has an invitation")
        .to_owned();
    cloud
        .post_empty(
            &format!("/v1/party/invitations/{invitation}/accept"),
            bea.token(),
        )
        .await
        .ok(200);
    let party = cloud.get("/v1/party", alec.token()).await.ok(200);
    assert_eq!(party.body["members"].as_array().map(Vec::len), Some(2));

    // The snapshot, while this cloud still has its database open and the write-ahead log in use.
    let archive = var("ANYPLAY_DRILL_ARCHIVE");
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = here.join("../../deploy/snapshot.py");
    let output = Command::new(std::env::var("ANYPLAY_DRILL_PYTHON").unwrap_or("python3".into()))
        .arg(&script)
        .arg(var("ANYPLAY_DRILL_DATA"))
        .arg(&archive)
        .output()
        .expect("python3 runs");
    assert!(
        output.status.success(),
        "snapshot.py failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // After the snapshot: this account must not be in the restored cloud.
    let late = cloud.sign_up("DrillLate").await;

    let state = json!({
        "alec": {"email": alec.email, "id": alec.id, "handle": alec.handle, "access": alec.token()},
        "bea": {"id": bea.id},
        "cy": {"id": cy.id, "email": cy.email},
        "late": {"id": late.id},
    });
    std::fs::write(var("ANYPLAY_DRILL_STATE"), state.to_string()).unwrap();
}

#[tokio::test]
async fn drill_verify() {
    if phase().as_deref() != Some("verify") {
        return;
    }
    let state: Value =
        serde_json::from_str(&std::fs::read_to_string(var("ANYPLAY_DRILL_STATE")).unwrap())
            .unwrap();
    let data = var("ANYPLAY_DRILL_DATA");
    let cloud = TestCloud::with(move |c| c.data_dir = data.into(), |_| {}).await;
    let alec = &state["alec"];
    let alec_id = alec["id"].as_u64().unwrap();
    let bea_id = state["bea"]["id"].as_u64().unwrap();
    let late_id = state["late"]["id"].as_i64().unwrap();

    // The signing key came back: a token issued before the backup is still good.
    let me = cloud
        .get("/v1/me", alec["access"].as_str().unwrap())
        .await
        .ok(200);
    assert_eq!(me.body["id"].as_u64(), Some(alec_id), "{}", me.body);

    // Sign-in by e-mail code against the restored accounts.
    let signed_in = cloud
        .sign_in(alec["email"].as_str().unwrap(), "after the restore")
        .await;
    assert_eq!(signed_in.me.id, alec_id);
    let token = signed_in.tokens.access;

    // Friends.
    let friends = cloud.get("/v1/friends", &token).await.ok(200);
    let listed: Vec<u64> = friends.body["friends"]
        .as_array()
        .expect("a friend list")
        .iter()
        .filter_map(|f| f["account"]["id"].as_u64())
        .collect();
    assert_eq!(listed, [bea_id], "friends: {}", friends.body);

    // Party.
    let party = cloud.get("/v1/party", &token).await.ok(200);
    let members: Vec<u64> = party.body["members"]
        .as_array()
        .expect("party members")
        .iter()
        .filter_map(|m| m["account"]["id"].as_u64().or_else(|| m["id"].as_u64()))
        .collect();
    assert!(
        members.contains(&alec_id) && members.contains(&bea_id) && members.len() == 2,
        "party: {}",
        party.body
    );

    // The point in time: an account made after the snapshot is not there, the third player is.
    let present: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts WHERE id = $1")
        .bind(late_id)
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(
        present, 0,
        "the account made after the snapshot is in the restore"
    );
    let cy: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts WHERE id = $1")
        .bind(state["cy"]["id"].as_i64().unwrap())
        .fetch_one(&cloud.state.db)
        .await
        .unwrap();
    assert_eq!(cy, 1);
}
