//! The events WebSocket.
//!
//! The bearer token is checked on the upgrade request. A request that carries no genuine token is
//! refused with 401. When the token is genuine but unusable (expired, device signed out, account
//! banned) the upgrade succeeds and the connection closes at once with the matching close code,
//! so a client that only sees WebSocket close codes still learns what to do.

use std::borrow::Cow;
use std::time::Duration;

use anyplay_api::{Event, close};
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;

use crate::error::AppError;
use crate::http::{Auth, AuthFailure, count_request, verify_access};
use crate::news::Push;
use crate::state::AppState;

/// Clients only answer pings; anything bigger is a mistake.
const MAX_CLIENT_MESSAGE: usize = 4096;
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

enum Verdict {
    Open(Auth),
    Refuse {
        /// Boxed: an event is far larger than the rest of the verdict.
        event: Option<Box<Event>>,
        code: u16,
        reason: &'static str,
    },
}

pub async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let verdict = match verify_access(&state, &headers) {
        Err(AuthFailure::Invalid) => {
            return Err(AppError::unauthorized(
                "Send a valid access token as a Bearer token.",
            ));
        }
        Err(AuthFailure::Expired) => Verdict::Refuse {
            event: None,
            code: close::TOKEN_EXPIRED,
            reason: "access token expired",
        },
        Ok(auth) => match state.check_device(auth.device, auth.account).await {
            Ok(()) => {
                count_request(&state, auth.account)?;
                Verdict::Open(auth)
            }
            Err(error) if error.body.error == anyplay_api::ErrorCode::Banned => Verdict::Refuse {
                event: Some(Box::new(Event::Banned {
                    until: error.body.until,
                })),
                code: close::BANNED,
                reason: "banned",
            },
            Err(_) => Verdict::Refuse {
                event: Some(Box::new(Event::SignedOut)),
                code: close::SIGNED_OUT,
                reason: "signed out",
            },
        },
    };
    Ok(upgrade
        .max_message_size(MAX_CLIENT_MESSAGE)
        .on_upgrade(move |socket| async move {
            match verdict {
                Verdict::Open(auth) => run(state, auth, socket).await,
                Verdict::Refuse {
                    event,
                    code,
                    reason,
                } => refuse(socket, event.map(|e| *e), code, reason).await,
            }
        }))
}

async fn send_event(socket: &mut WebSocket, event: &Event) -> bool {
    let Ok(text) = serde_json::to_string(event) else {
        return false;
    };
    matches!(
        tokio::time::timeout(SEND_TIMEOUT, socket.send(Message::Text(text.into()))).await,
        Ok(Ok(()))
    )
}

async fn send_close(socket: &mut WebSocket, code: u16, reason: Cow<'static, str>) {
    let frame = CloseFrame {
        code,
        reason: reason.into_owned().into(),
    };
    let _ = tokio::time::timeout(SEND_TIMEOUT, socket.send(Message::Close(Some(frame)))).await;
}

async fn refuse(mut socket: WebSocket, event: Option<Event>, code: u16, reason: &'static str) {
    if let Some(event) = event {
        send_event(&mut socket, &event).await;
    }
    send_close(&mut socket, code, reason.into()).await;
}

async fn run(state: AppState, auth: Auth, mut socket: WebSocket) {
    let (mut pushes, serial) = state.connect(auth.account, auth.device).await;
    let period = state.tuning.ping_interval;
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    let mut awaiting_pong = false;

    loop {
        tokio::select! {
            push = pushes.recv() => match push {
                Some(Push::Event(event)) => {
                    if !send_event(&mut socket, &event).await {
                        break;
                    }
                }
                Some(Push::Close { code, reason }) => {
                    send_close(&mut socket, code, reason).await;
                    break;
                }
                None => break,
            },
            frame = socket.recv() => match frame {
                // Any frame, a pong above all, shows the client is alive.
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => awaiting_pong = false,
            },
            _ = ping.tick() => {
                if awaiting_pong {
                    tracing::debug!(device = auth.device, "no pong; dropping the connection");
                    break;
                }
                awaiting_pong = true;
                let sent = tokio::time::timeout(
                    SEND_TIMEOUT,
                    socket.send(Message::Ping(Vec::new().into())),
                ).await;
                if !matches!(sent, Ok(Ok(()))) {
                    break;
                }
            }
        }
    }
    state.disconnect(auth.account, auth.device, serial).await;
}
