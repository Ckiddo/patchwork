use prost::Message;
use std::{cell::RefCell, rc::Rc};
use util_lib::protocol::{
    self,
    v1::{self, client_envelope, lobby_request::Command, server_envelope::Payload},
};
use wasm_bindgen::prelude::*;
use web_sys::{CustomEvent, HtmlInputElement};
use yew::prelude::*;

#[wasm_bindgen(module = "/src/browser_session.mjs")]
extern "C" {
    #[wasm_bindgen(js_name=noteSessionPong)]
    fn note_pong();
    #[wasm_bindgen(catch,js_name=sendSessionMessage)]
    fn send_message(bytes: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(js_name=newRequestId)]
    fn new_id() -> String;
    #[wasm_bindgen(catch,js_name=saveRoomPending)]
    fn save_pending(key: &str, value: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(catch,js_name=loadRoomPending)]
    fn load_pending(key: &str) -> Result<Option<String>, JsValue>;
    #[wasm_bindgen(catch,js_name=clearRoomPending)]
    fn clear_pending(key: &str) -> Result<(), JsValue>;
}
#[derive(Default)]
struct View {
    room: Option<v1::RoomSnapshot>,
    rooms: Vec<v1::RoomSnapshot>,
    next: String,
    pending: Option<v1::ClientEnvelope>,
    error: Option<String>,
    game: Option<serde_json::Value>,
    game_version: u64,
    game_seq: u64,
    game_phase: String,
    synced: bool,
    connected: bool,
    sync_request: Option<v1::ClientEnvelope>,
    context_request: Option<String>,
    pending_sent: bool,
    auto_replay: bool,
}
fn checked_game_state(
    value: serde_json::Value,
    game_id: &str,
    phase: &str,
) -> Option<serde_json::Value> {
    use game_core::{
        rules::registry::GAME_SNAPSHOT_KIND,
        state::{GameSnapshot, Lifecycle, ResultReason},
    };
    if value["game_id"].as_str() != Some(game_id) {
        return None;
    }
    if value["kind"] == GAME_SNAPSHOT_KIND {
        let state: GameSnapshot = serde_json::from_value(value.clone()).ok()?;
        let expected = match state.lifecycle() {
            Lifecycle::Running => "playing",
            Lifecycle::Paused => "paused",
            Lifecycle::Finished
                if state
                    .result()
                    .is_some_and(|r| r.reason == ResultReason::Abandoned) =>
            {
                "abandoned"
            }
            Lifecycle::Finished => "finished",
        };
        if phase != expected {
            return None;
        }
    } else if value["kind"] != game_core::rules::registry::LEGACY_SNAPSHOT_KIND {
        return None;
    }
    Some(value)
}
fn drive_sync(state: &Rc<RefCell<View>>, full: bool) {
    let mut view = state.borrow_mut();
    let lobby_pending = view
        .pending
        .as_ref()
        .is_some_and(|m| !matches!(m.payload, Some(client_envelope::Payload::Game(_))));
    if !view.connected
        || lobby_pending
        || view.sync_request.is_some()
        || view.context_request.is_some()
        || view.synced
    {
        return;
    }
    let Some(room) = &view.room else { return };
    if room.game_id.is_empty() {
        return;
    }
    let message = v1::ClientEnvelope {
        protocol_version: protocol::VERSION,
        request_id: new_id(),
        payload: Some(client_envelope::Payload::Resume(v1::ResumeRequest {
            game_id: room.game_id.clone(),
            last_seq: if full { 0 } else { view.game_seq },
            has_snapshot: !full && view.game.is_some(),
        })),
    };
    if send_message(&message.encode_to_vec()).is_ok() {
        view.sync_request = Some(message);
    }
}
fn error_text(code: i32) -> &'static str {
    match v1::ErrorCode::try_from(code).ok() {
        Some(v1::ErrorCode::RoomFull) => "房间已满。",
        Some(v1::ErrorCode::RoomNotJoinable) => "当前房间阶段不允许此操作。",
        Some(v1::ErrorCode::NotEnoughPlayers) => "需要两名玩家才能开局。",
        Some(v1::ErrorCode::NotReady) => "两名玩家都需要在线并准备。",
        Some(v1::ErrorCode::VersionConflict) => "房间已变化，请刷新房间后再操作。",
        Some(v1::ErrorCode::Forbidden) => "当前身份没有执行此操作的权限。",
        Some(v1::ErrorCode::NotFound) => "房间不存在，或当前没有加入房间。",
        Some(v1::ErrorCode::BadPassword) => "房间密码不正确。",
        Some(v1::ErrorCode::PlayerBusy) => "已有房间、排队或待确认操作，请稍后重试。",
        Some(v1::ErrorCode::RequestIdConflict) => "请求编号已用于不同操作。",
        Some(v1::ErrorCode::ServiceUnavailable) => "服务暂不可用。操作记录已保留，可重试原请求。",
        Some(v1::ErrorCode::GameNotRunning) => "对局已暂停或结束，当前不能行动。",
        Some(v1::ErrorCode::NotYourTurn) => "现在轮到对手行动。",
        Some(v1::ErrorCode::InvalidPlacement) => "落点越界、重叠或拼布已经放置。",
        Some(v1::ErrorCode::InsufficientButtons) => "纽扣不足，无法购买这块拼布。",
        Some(v1::ErrorCode::WrongActionPhase) => "请先完成当前待放的特殊拼布。",
        Some(v1::ErrorCode::SyncRequired) => "正在等待双方完成对局同步。",
        _ => "请求无效或该功能尚未开放。",
    }
}
fn transmit(state: &Rc<RefCell<View>>, key: &str, message: v1::ClientEnvelope) {
    let bytes = message.encode_to_vec();
    let saved = serde_json::to_string(&bytes)
        .ok()
        .is_some_and(|v| save_pending(key, &v).is_ok());
    if !saved {
        state.borrow_mut().error = Some("无法保存操作记录，未发送请求。".into());
        return;
    }
    state.borrow_mut().pending = Some(message);
    state.borrow_mut().pending_sent = false;
    state.borrow_mut().auto_replay = true;
    state.borrow_mut().error = None;
    replay_pending(state);
}
fn replay_pending(state: &Rc<RefCell<View>>) {
    let mut view = state.borrow_mut();
    if !view.connected || view.pending_sent || !view.auto_replay {
        return;
    }
    let Some(message) = view.pending.as_ref() else {
        return;
    };
    if matches!(message.payload, Some(client_envelope::Payload::Game(_))) && !view.synced {
        return;
    }
    let bytes = message.encode_to_vec();
    if send_message(&bytes).is_ok() {
        view.pending_sent = true;
    } else {
        view.error = Some("连接不可用，操作记录已保留，请重新连接。".into());
    }
}
fn load_context(state: &Rc<RefCell<View>>) {
    let message = envelope(String::new(), 0, Command::Get(v1::Empty {}));
    if send_message(&message.encode_to_vec()).is_ok() {
        state.borrow_mut().context_request = Some(message.request_id);
    }
}
fn envelope(room: String, version: u64, command: Command) -> v1::ClientEnvelope {
    v1::ClientEnvelope {
        protocol_version: protocol::VERSION,
        request_id: new_id(),
        payload: Some(client_envelope::Payload::Lobby(v1::LobbyRequest {
            room_id: room,
            expected_version: version,
            command: Some(command),
        })),
    }
}
#[derive(Properties, PartialEq)]
pub struct Props {
    pub user_id: String,
    pub connected: bool,
    pub connection_epoch: u32,
}
#[function_component(FriendRooms)]
pub fn friend_rooms(props: &Props) -> Html {
    let state = use_mut_ref(View::default);
    let redraw = use_force_update();
    let password = use_state(String::new);
    let code = use_state(String::new);
    let rules = use_state(|| game_core::rules::CUSTOM_RULES_VERSION.to_string());
    let key = format!(
        "patchwork_room_pending_v1:{}:{}",
        crate::app::jwt_base_url(),
        props.user_id
    );
    {
        let state = state.clone();
        let redraw = redraw.clone();
        let key = key.clone();
        let user = props.user_id.clone();
        use_effect_with((), move |_| {
            let event_state = state.clone();
            let event_key = key.clone();
            let event_redraw = redraw.clone();
            let callback = Closure::<dyn FnMut(web_sys::Event)>::new(
                move |event: web_sys::Event| {
                    let Some(event) = event.dyn_ref::<CustomEvent>() else {
                        return;
                    };
                    let bytes = js_sys::Uint8Array::new(&event.detail()).to_vec();
                    let Ok(message) = v1::ServerEnvelope::decode(bytes.as_slice()) else {
                        return;
                    };
                    if message.protocol_version != protocol::VERSION {
                        return;
                    }
                    if matches!(&message.payload, Some(Payload::Pong(p)) if p.nonce == 1) {
                        note_pong();
                        return;
                    }
                    let mut view = event_state.borrow_mut();
                    let sync_reply = message.request_id.as_ref().is_some_and(|id| {
                        view.sync_request
                            .as_ref()
                            .is_some_and(|r| r.request_id == *id)
                    });
                    let mut full_sync = false;
                    let mut retry_sync = false;
                    if sync_reply {
                        view.sync_request = None;
                    }
                    let is_reply = message.request_id.as_ref().is_some_and(|id| {
                        view.pending.as_ref().is_some_and(|p| p.request_id == *id)
                    });
                    let context_reply = message
                        .request_id
                        .as_ref()
                        .is_some_and(|id| view.context_request.as_ref() == Some(id));
                    if context_reply {
                        view.context_request = None;
                    }
                    let was_get=context_reply||view.pending.as_ref().is_some_and(|m|matches!(&m.payload,Some(client_envelope::Payload::Lobby(l)) if matches!(l.command,Some(Command::Get(_)))));
                    let game_reply = is_reply
                        && view.pending.as_ref().is_some_and(|m| {
                            matches!(m.payload, Some(client_envelope::Payload::Game(_)))
                        });
                    let sync_required = game_reply
                        && matches!(&message.payload,Some(Payload::Error(e)) if e.code==v1::ErrorCode::SyncRequired as i32);
                    let retryable =
                        matches!(&message.payload,Some(Payload::Error(e)) if e.retryable);
                    let list_after = was_get
                        && (is_reply || context_reply)
                        && !view.pending.as_ref().is_some_and(|m| {
                            matches!(m.payload, Some(client_envelope::Payload::Game(_)))
                        })
                        && matches!(&message.payload,Some(Payload::Error(e)) if e.code==v1::ErrorCode::NotFound as i32);
                    if is_reply && !retryable && !sync_required {
                        if clear_pending(&event_key).is_ok() {
                            view.pending = None;
                            view.pending_sent = false;
                        } else {
                            view.error =
                                Some("操作已响应，但无法清理本地记录；重新加载会安全重试。".into());
                        }
                    }
                    match message.payload {
                        Some(Payload::Room(room)) => {
                            if room.members.iter().any(|m| m.user_id == user) {
                                if view.room.as_ref().is_none_or(|r| {
                                    r.room_id != room.room_id || room.version >= r.version
                                }) {
                                    if view.room.as_ref().is_none_or(|r| r.game_id != room.game_id)
                                    {
                                        view.game = None;
                                        view.game_version = 0;
                                        view.game_seq = 0;
                                        view.synced = false;
                                        view.sync_request = None;
                                    }
                                    view.room = Some(room);
                                }
                            } else if view
                                .room
                                .as_ref()
                                .is_some_and(|r| r.room_id == room.room_id)
                            {
                                view.room = None;
                                view.game = None;
                                view.synced = false;
                                view.sync_request = None;
                            }
                            if is_reply {
                                view.error = None;
                            }
                        }
                        Some(Payload::Rooms(list)) => {
                            view.rooms = list.rooms;
                            view.next = list.next_cursor;
                        }
                        Some(Payload::Game(game)) => {
                            if view
                                .room
                                .as_ref()
                                .is_some_and(|r| r.game_id == game.game_id)
                            {
                                if game.version >= view.game_version {
                                    view.game =
                                        serde_json::from_slice(&game.state_json).ok().and_then(
                                            |v| checked_game_state(v, &game.game_id, &game.phase),
                                        );
                                    if view.game.is_none() {
                                        view.synced = false;
                                        full_sync = true;
                                    }
                                    view.game_version = game.version;
                                    view.game_seq = game.event_seq;
                                    view.game_phase = game.phase;
                                }
                            }
                        }
                        Some(Payload::Resumed(resumed)) if sync_reply => {
                            if view
                                .room
                                .as_ref()
                                .is_some_and(|r| r.game_id == resumed.game_id)
                            {
                                let mut state = view.game.clone();
                                if let Some(snapshot) = resumed.snapshot {
                                    state = if snapshot.game_id == resumed.game_id
                                        && snapshot.version == resumed.version
                                        && snapshot.event_seq == resumed.event_seq
                                    {
                                        serde_json::from_slice(&snapshot.state_json).ok()
                                    } else {
                                        None
                                    };
                                } else {
                                    let mut seq = view.game_seq;
                                    let mut version = view.game_version;
                                    for event in resumed.events {
                                        let payload = serde_json::from_slice::<serde_json::Value>(
                                            &event.payload_json,
                                        )
                                        .ok();
                                        if event.seq != seq + 1
                                            || event.version != version + 1
                                            || !payload.as_ref().is_some_and(|p| {
                                                p["kind"] == "connection_state_v1"
                                                    || p["kind"] == "game_transition_v1"
                                            })
                                        {
                                            state = None;
                                            break;
                                        }
                                        state = payload.and_then(|p| p.get("state").cloned());
                                        seq = event.seq;
                                        version = event.version;
                                    }
                                    if seq != resumed.event_seq || version != resumed.version {
                                        state = None;
                                    }
                                }
                                state = state.and_then(|v| {
                                    checked_game_state(v, &resumed.game_id, &resumed.phase)
                                });
                                if state.is_some() && resumed.version >= view.game_version {
                                    view.game = state;
                                    view.game_version = resumed.version;
                                    view.game_seq = resumed.event_seq;
                                    view.game_phase = resumed.phase;
                                    let ack = v1::ClientEnvelope {
                                        protocol_version: protocol::VERSION,
                                        request_id: new_id(),
                                        payload: Some(client_envelope::Payload::SyncAck(
                                            v1::SyncAck {
                                                game_id: resumed.game_id,
                                                version: resumed.version,
                                                event_seq: resumed.event_seq,
                                                sync_token: resumed.sync_token,
                                            },
                                        )),
                                    };
                                    if send_message(&ack.encode_to_vec()).is_ok() {
                                        view.sync_request = Some(ack);
                                    }
                                } else {
                                    view.synced = false;
                                    full_sync = true;
                                }
                            }
                        }
                        Some(Payload::Acknowledged(ack)) if sync_reply => {
                            view.synced = ack.game_version == view.game_version;
                            if view.synced {
                                view.error = None;
                            }
                        }
                        Some(Payload::Acknowledged(_)) if game_reply => {
                            view.error = None;
                        }
                        Some(Payload::Error(e)) if sync_reply => {
                            view.synced = false;
                            view.error = Some("正在重新同步已提交的对局版本。".into());
                            retry_sync = e.retryable;
                            full_sync = true;
                        }
                        Some(Payload::Error(e)) if is_reply || context_reply => {
                            if sync_required {
                                view.synced = false;
                                view.auto_replay = false;
                                view.pending_sent = false;
                                full_sync = true;
                            } else if game_reply && e.code == v1::ErrorCode::VersionConflict as i32
                            {
                                view.synced = false;
                                full_sync = true;
                            }
                            view.error = if list_after {
                                None
                            } else {
                                Some(error_text(e.code).into())
                            };
                        }
                        _ => {}
                    }
                    drop(view);
                    if list_after {
                        transmit(
                            &event_state,
                            &event_key,
                            envelope(
                                String::new(),
                                0,
                                Command::List(v1::ListRooms {
                                    cursor: String::new(),
                                    limit: 10,
                                }),
                            ),
                        );
                    }
                    if retry_sync {
                        let retry_state = event_state.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            crate::browser_session::reconnect_delay(1).await;
                            drive_sync(&retry_state, true);
                        });
                    } else {
                        drive_sync(&event_state, full_sync);
                    }
                    replay_pending(&event_state);
                    event_redraw.force_update();
                },
            );
            let window = web_sys::window().expect("browser");
            let _ = window.add_event_listener_with_callback(
                "patchwork-message",
                callback.as_ref().unchecked_ref(),
            );
            redraw.force_update();
            move || {
                let _ = window.remove_event_listener_with_callback(
                    "patchwork-message",
                    callback.as_ref().unchecked_ref(),
                );
            }
        });
    }
    {
        let state = state.clone();
        let key = key.clone();
        let redraw = redraw.clone();
        use_effect_with(
            (props.connected, props.connection_epoch),
            move |(connected, _)| {
                {
                    let mut view = state.borrow_mut();
                    view.connected = *connected;
                    view.synced = false;
                    view.sync_request = None;
                    view.context_request = None;
                    view.pending_sent = false;
                    view.auto_replay = true;
                }
                if *connected {
                    match load_pending(&key) {
                        Ok(Some(saved)) => {
                            let pending = serde_json::from_str::<Vec<u8>>(&saved)
                                .ok()
                                .and_then(|bytes| protocol::decode_client(&bytes).ok());
                            if let Some(pending) = pending {
                                if matches!(
                                    pending.payload,
                                    Some(client_envelope::Payload::Game(_))
                                ) {
                                    state.borrow_mut().pending = Some(pending);
                                    load_context(&state);
                                } else {
                                    transmit(&state, &key, pending);
                                }
                            } else {
                                state.borrow_mut().error =
                                    Some("本地房间操作记录无效，未自动覆盖。".into());
                            }
                        }
                        Ok(None) => transmit(
                            &state,
                            &key,
                            envelope(String::new(), 0, Command::Get(v1::Empty {})),
                        ),
                        Err(_) => {
                            state.borrow_mut().error = Some("无法读取本地操作记录。".into());
                        }
                    }
                }
                redraw.force_update();
                || ()
            },
        );
    }
    let act = {
        let state = state.clone();
        let key = key.clone();
        let redraw = redraw.clone();
        Callback::from(move |(room, version, command): (String, u64, Command)| {
            if state.borrow().pending.is_some() || !state.borrow().connected {
                return;
            }
            transmit(&state, &key, envelope(room, version, command));
            redraw.force_update();
        })
    };
    let retry = {
        let state = state.clone();
        let key = key.clone();
        let redraw = redraw.clone();
        Callback::from(move |_| {
            let pending = state.borrow().pending.clone();
            if let Some(pending) = pending {
                transmit(&state, &key, pending);
                drive_sync(&state, false);
                redraw.force_update();
            }
        })
    };
    let on_password = {
        let password = password.clone();
        Callback::from(move |e: InputEvent| {
            password.set(e.target_unchecked_into::<HtmlInputElement>().value())
        })
    };
    let on_code = {
        let code = code.clone();
        Callback::from(move |e: InputEvent| {
            code.set(e.target_unchecked_into::<HtmlInputElement>().value())
        })
    };
    let on_rules = {
        let rules = rules.clone();
        Callback::from(move |e: InputEvent| {
            rules.set(e.target_unchecked_into::<HtmlInputElement>().value())
        })
    };
    let create = {
        let act = act.clone();
        let password = password.clone();
        let rules = rules.clone();
        Callback::from(move |_| {
            act.emit((
                String::new(),
                0,
                Command::Create(v1::CreateRoom {
                    mode: "casual".into(),
                    rules_version: (*rules).clone(),
                    password: if password.is_empty() {
                        None
                    } else {
                        Some((*password).clone())
                    },
                }),
            ))
        })
    };
    let join_code = {
        let act = act.clone();
        let code = code.clone();
        let password = password.clone();
        Callback::from(move |_| {
            act.emit((
                String::new(),
                0,
                Command::Join(v1::JoinRoom {
                    code: code.trim().to_ascii_uppercase(),
                    password: Some((*password).clone()),
                }),
            ))
        })
    };
    let refresh = {
        let act = act.clone();
        let state = state.clone();
        Callback::from(move |_| {
            let room = state.borrow().room.clone();
            act.emit((
                room.as_ref().map(|r| r.room_id.clone()).unwrap_or_default(),
                0,
                if room.is_some() {
                    Command::Get(v1::Empty {})
                } else {
                    Command::List(v1::ListRooms {
                        cursor: String::new(),
                        limit: 10,
                    })
                },
            ));
        })
    };
    let view = state.borrow();
    let busy = view.pending.is_some() || !props.connected || view.sync_request.is_some();
    let game_snapshot = view
        .game
        .clone()
        .and_then(|s| serde_json::from_value::<game_core::state::GameSnapshot>(s).ok())
        .map(Rc::new);
    let game_action = {
        let state = state.clone();
        let key = key.clone();
        let redraw = redraw.clone();
        Callback::from(move |request: v1::GameRequest| {
            let view = state.borrow();
            if view.pending.is_some()
                || !view.connected
                || !view.synced
                || view.sync_request.is_some()
                || request.expected_version != view.game_version
                || !view
                    .room
                    .as_ref()
                    .is_some_and(|r| r.game_id == request.game_id)
            {
                return;
            }
            drop(view);
            transmit(
                &state,
                &key,
                v1::ClientEnvelope {
                    protocol_version: protocol::VERSION,
                    request_id: new_id(),
                    payload: Some(client_envelope::Payload::Game(request)),
                },
            );
            redraw.force_update();
        })
    };
    let leave_game = {
        let act = act.clone();
        let state = state.clone();
        Callback::from(move |()| {
            let room = state.borrow().room.clone();
            if let Some(r) = room {
                act.emit((r.room_id, r.version, Command::Leave(v1::Empty {})));
            }
        })
    };
    let names = std::array::from_fn(|seat| {
        view.room
            .as_ref()
            .and_then(|r| r.members.iter().find(|m| m.seat as usize == seat))
            .map(|m| m.nickname.clone())
            .unwrap_or_else(|| format!("玩家 {}", seat + 1))
    });
    html! {
        <>
        <section class="friend-rooms" data-testid="friend-rooms"
            aria-busy={(props.connected && (view.pending.is_some() || view.sync_request.is_some())).to_string()}>
            <div class="room-toolbar">
                <h2>{"好友房间"}</h2>
                <button onclick={refresh} disabled={busy}>{"刷新"}</button>
            </div>
            // Keep request feedback in one fixed slot so round trips never move the controls.
            <div class="room-feedback">
                <p class="room-note">{"邀请一位朋友，双方准备后由房主开局。"}</p>
                if let Some(error)=&view.error {
                    <div class="room-feedback-layer room-error" role="alert">
                        <p>{error}</p>
                        if view.pending.is_some() {
                            <button disabled={!props.connected} onclick={retry.clone()}>{"重试原请求"}</button>
                        }
                    </div>
                } else if view.pending.is_some() {
                    <div class="room-feedback-layer room-pending" role="status">
                        <p>{"正在确认操作…"}</p>
                        <button disabled={!props.connected} onclick={retry}>{"重试原请求"}</button>
                    </div>
                }
            </div>
            if let Some(room)=&view.room {
                <div class="room-heading">
                    <strong>{"房间码："}<span id="room-code">{&room.code}</span></strong>
                    <span>{if room.phase==v1::RoomPhase::Playing as i32{"对局已创建"}else{"等待准备"}}</span>
                </div>
                <div class="room-seats">
                    {for (0..2u32).map(|seat|{
                        let member=room.members.iter().find(|m|m.seat==seat);
                        html!{<div class="room-seat" key={seat} data-seat={seat.to_string()}>
                            <strong>{format!("座位 {}",seat+1)}</strong>
                            if let Some(member)=member {
                                <p>{&member.nickname}{if member.user_id==props.user_id{"（你）"}else{""}}</p>
                                <p>{if member.user_id==room.owner_id{"房主 · "}else{""}}{if member.connected{"在线"}else{"离线"}}</p>
                                <p>{if member.ready{"已准备"}else{"未准备"}}</p>
                            } else {<p>{"等待朋友加入"}</p>}
                        </div>}
                    })}
                </div>
                if room.phase==v1::RoomPhase::Waiting as i32 {
                    <div class="room-actions">
                        <button disabled={busy} onclick={
                            let act=act.clone();let room=room.clone();let user=props.user_id.clone();
                            Callback::from(move |_|{let ready=!room.members.iter().any(|m|m.user_id==user&&m.ready);act.emit((room.room_id.clone(),room.version,Command::SetReady(v1::SetReady{ready})));})
                        }>{if room.members.iter().any(|m|m.user_id==props.user_id&&m.ready){"取消准备"}else{"准备"}}</button>
                        if room.owner_id==props.user_id {
                            <button disabled={busy} onclick={let act=act.clone();let room=room.clone();Callback::from(move |_|act.emit((room.room_id.clone(),room.version,Command::Start(v1::Empty{}))))}>{"开始对局"}</button>
                        }
                        <button disabled={busy} onclick={let act=act.clone();let room=room.clone();Callback::from(move |_|act.emit((room.room_id.clone(),room.version,Command::Leave(v1::Empty{}))))}>{"离开房间"}</button>
                    </div>
                } else {
                    <p id="shared-game-id">{format!("共同对局：{}",room.game_id)}</p>
                    <p>{room.first_player_seat.map(|s|format!("先手：座位 {}",s+1)).unwrap_or_default()}</p>
                    <p role="status">{if !props.connected {"连接中断，等待恢复。"} else if !view.synced {"正在同步对局，操作暂不可用。"} else if view.game_phase=="paused" {"对局已同步，等待双方恢复连接。"} else if view.game_phase=="abandoned" {"双方离线超时，对局已无胜者结束。"} else if view.game_phase=="finished" {"对局已结束。"} else if game_snapshot.is_none(){"这是历史演示房间，请离开后创建正式好友房。"}else {"对局已同步，请在图版右侧操作。"}}</p>
                    <p>{format!("对局版本 {} · 事件序号 {}",view.game_version,view.game_seq)}</p>
                    if room.phase==v1::RoomPhase::Finished as i32 {
                        <button disabled={busy} onclick={let act=act.clone();let room=room.clone();Callback::from(move |_|act.emit((room.room_id.clone(),room.version,Command::Leave(v1::Empty{}))))}>{"离开房间"}</button>
                    }
                }
                <details class="room-details">
                <summary>{"房间设置与版本"}</summary>
                <p id="room-version">{format!("房间版本 {} · 规则 {}",room.version,room.rules_version)}</p>
                if room.owner_id==props.user_id && room.phase==v1::RoomPhase::Waiting as i32 {
                    <label>{"规则标识"}<input value={(*rules).clone()} oninput={on_rules.clone()} maxlength="64" disabled={busy}/></label>
                    <button disabled={busy} onclick={let act=act.clone();let room=room.clone();let rules=rules.clone();Callback::from(move |_|act.emit((room.room_id.clone(),room.version,Command::SetRules(v1::SetRules{rules_version:(*rules).clone()}))))}>{"更新规则并重置准备"}</button>
                }
                </details>
            } else {
                <div class="room-entry">
                    <label>{"房间密码（可选）"}<input type="password" value={(*password).clone()} oninput={on_password} maxlength="128" disabled={busy}/></label>
                    <button onclick={create} disabled={busy}>{"创建好友房"}</button>
                    <label>{"房间码"}<input value={(*code).clone()} oninput={on_code} maxlength="10" disabled={busy}/></label>
                    <button onclick={join_code} disabled={busy}>{"按房间码加入"}</button>
                </div>
                <ul class="room-list">
                    {for view.rooms.iter().map(|room|html!{<li key={room.room_id.clone()}>
                        <span>{format!("{} · {}/2 人{}",room.code,room.members.len(),if room.requires_password{" · 需要密码"}else{""})}</span>
                        <button class={classes!((room.members.len()>=2).then_some("unavailable"))} disabled={busy||room.members.len()>=2} onclick={
                            let act=act.clone();let room=room.clone();let password=password.clone();
                            Callback::from(move |_|act.emit((room.room_id.clone(),room.version,Command::Join(v1::JoinRoom{code:room.code.clone(),password:Some((*password).clone())}))))
                        }>{"加入"}</button>
                    </li>})}
                </ul>
                if !view.next.is_empty() {
                    <button disabled={busy} onclick={let act=act.clone();let next=view.next.clone();Callback::from(move |_|act.emit((String::new(),0,Command::List(v1::ListRooms{cursor:next.clone(),limit:10}))))}>{"下一页"}</button>
                }
            }
        </section>
        <crate::game_view::GameTable snapshot={game_snapshot} user_id={props.user_id.clone()} names={names} version={view.game_version} connected={props.connected} synced={view.synced} busy={busy} on_action={game_action} on_leave={leave_game}/>
        </>
    }
}
