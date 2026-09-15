//! Formal game UI. Only server snapshots are rendered as committed pieces or resources.
use game_core::{
    BoardPosition, Seat,
    actions::{ActionError, GameAction},
    geometry::{Orientation, distinct_orientations},
    rules::{CUSTOM_V1, PatchId, patch},
    state::{
        ActionPhase, GameSnapshot, Lifecycle, Outcome, PieceId, PlayerState, SpecialPatchStatus,
    },
};
use std::rc::Rc;
use util_lib::protocol::v1;
use wasm_bindgen::{JsCast, closure::Closure};
use yew::prelude::*;

#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) struct Draft {
    pub patch: Option<PatchId>,
    pub anchor: Option<BoardPosition>,
    pub orientation: Orientation,
}
#[derive(Clone, Copy)]
enum Control {
    Left,
    Right,
    Flip,
    Cancel,
    Confirm,
    Advance,
    Anchor(BoardPosition),
    PlaceAt(BoardPosition),
    Select(PatchId),
    Move(i32, i32),
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub snapshot: Option<Rc<GameSnapshot>>,
    pub user_id: String,
    pub names: [String; 2],
    pub version: u64,
    pub connected: bool,
    pub synced: bool,
    pub busy: bool,
    pub on_action: Callback<v1::GameRequest>,
    pub on_leave: Callback<()>,
}
fn seat_color(seat: Seat) -> &'static str {
    if seat == Seat::First {
        "#63d8de"
    } else {
        "#f59fb7"
    }
}
fn special_for(state: &GameSnapshot, seat: Seat) -> Option<u8> {
    match state.action_phase() {
        ActionPhase::Special {
            actor,
            track_position,
        } if actor == seat => Some(track_position),
        _ => None,
    }
}
fn intent(state: &GameSnapshot, seat: Seat, draft: Draft) -> Option<GameAction> {
    let position = draft.anchor?;
    Some(if special_for(state, seat).is_some() {
        GameAction::PlaceSpecialPatch { position }
    } else {
        GameAction::BuyAndPlace {
            patch_id: draft.patch?,
            position,
            orientation: draft.orientation,
        }
    })
}
fn wire(state: &GameSnapshot, version: u64, action: GameAction) -> v1::GameRequest {
    fn pos(p: BoardPosition) -> v1::BoardPosition {
        let (x, y) = p.coordinates();
        v1::BoardPosition {
            x: x.into(),
            y: y.into(),
        }
    }
    let action = match action {
        GameAction::Advance => v1::game_request::Action::Advance(v1::Empty {}),
        GameAction::PlaceSpecialPatch { position } => {
            v1::game_request::Action::PlaceSpecialPatch(pos(position))
        }
        GameAction::BuyAndPlace {
            patch_id,
            position,
            orientation,
        } => v1::game_request::Action::BuyAndPlace(v1::PlacePatch {
            patch_id: patch_id.0.to_string(),
            position: Some(pos(position)),
            quarter_turns: orientation.quarter_turns().into(),
            flipped: orientation.flipped(),
        }),
    };
    v1::GameRequest {
        game_id: state.game_id().into(),
        expected_version: version,
        action: Some(action),
    }
}
fn problem(error: ActionError) -> &'static str {
    match error {
        ActionError::InsufficientButtons { .. } => "纽扣不足，先选择前进或其他拼布。",
        ActionError::NotYourTurn => "等待对手行动。",
        ActionError::NotRunning | ActionError::GameFinished => "等待对局恢复，或对局已结束。",
        ActionError::WrongPhase => "请先放置已领取的特殊拼布。",
        _ => "此落点越界或重叠，请调整位置。",
    }
}

#[function_component(GameTable)]
pub fn game_table(props: &Props) -> Html {
    let draft = use_state_eq(Draft::default);
    let show_result = use_state_eq(|| true);
    let marker_note = use_state_eq(String::new);
    // Diagnostic checksum for comparing the entire committed snapshot across browsers.
    // Not an authentication or integrity boundary; draft changes do not recompute it.
    let state_checksum = use_memo(props.snapshot.clone(), |snapshot| {
        snapshot
            .as_ref()
            .map(|state| {
                let bytes =
                    serde_json::to_vec(&**state).expect("validated snapshot is serializable");
                let hash = bytes
                    .into_iter()
                    .fold(0xcbf29ce484222325_u64, |hash, byte| {
                        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
                    });
                format!("{hash:016x}")
            })
            .unwrap_or_default()
    });
    let placeable = use_memo(
        (props.snapshot.clone(), props.user_id.clone()),
        |(snapshot, user)| {
            let Some(state) = snapshot else {
                return Vec::new();
            };
            let Some((player, _)) = state.perspective(user) else {
                return Vec::new();
            };
            state
                .supply()
                .candidates()
                .into_iter()
                .filter(|&id| has_space(player, id))
                .collect::<Vec<_>>()
        },
    );
    let game_key = props.snapshot.as_ref().map(|s| s.game_id().to_string());
    {
        let draft = draft.clone();
        let marker_note = marker_note.clone();
        use_effect_with((game_key.clone(), props.version), move |_| {
            draft.set(Draft::default());
            // Income totals in a clicked track note belong to that committed snapshot.
            marker_note.set(String::new());
            || ()
        });
    }
    {
        let show_result = show_result.clone();
        use_effect_with(game_key, move |_| {
            show_result.set(true);
            || ()
        });
    }
    let control = {
        let draft = draft.clone();
        let snapshot = props.snapshot.clone();
        let user = props.user_id.clone();
        let emit = props.on_action.clone();
        let version = props.version;
        let unlocked = props.connected && props.synced && !props.busy;
        Callback::from(move |c: Control| {
            // Cancelling a draft never withdraws or changes an in-flight server request.
            if matches!(c, Control::Cancel) {
                draft.set(Draft::default());
                return;
            }
            let Some(state) = snapshot.as_ref() else {
                return;
            };
            let Some((player, _)) = state.perspective(&user) else {
                return;
            };
            let seat = player.seat();
            if !unlocked || state.input_actor() != Some(seat) {
                return;
            }
            let special = special_for(state, seat).is_some();
            let mut next = *draft;
            match c {
                Control::Select(id) if !special && state.supply().candidates().contains(&id) => {
                    next = Draft {
                        patch: Some(id),
                        ..Draft::default()
                    }
                }
                Control::Anchor(p) if special || next.patch.is_some() => next.anchor = Some(p),
                Control::Left if !special => {
                    next.orientation = next.orientation.rotate_counterclockwise()
                }
                Control::Right if !special => {
                    next.orientation = next.orientation.rotate_clockwise()
                }
                Control::Flip if !special => next.orientation = next.orientation.flip(),
                Control::Move(dx, dy) if special || next.patch.is_some() => {
                    let (x, y) = next
                        .anchor
                        .unwrap_or(BoardPosition::new(0, 0).unwrap())
                        .coordinates();
                    next.anchor = BoardPosition::new(
                        (i32::from(x) + dx).clamp(0, 8),
                        (i32::from(y) + dy).clamp(0, 8),
                    )
                    .ok();
                }
                Control::Confirm | Control::PlaceAt(_) => {
                    if let Control::PlaceAt(position) = c {
                        if !special && next.patch.is_none() {
                            return;
                        }
                        // Validate the clicked cell directly, without waiting for a hover render.
                        next.anchor = Some(position);
                        draft.set(next);
                    }
                    if let Some(action) = intent(state, seat, next) {
                        if state.apply_action(&user, action).is_ok() {
                            emit.emit(wire(state, version, action));
                        }
                    }
                    return;
                }
                Control::Advance if !special => {
                    if state.apply_action(&user, GameAction::Advance).is_ok() {
                        emit.emit(wire(state, version, GameAction::Advance));
                    }
                    return;
                }
                _ => return,
            }
            draft.set(next);
        })
    };
    // A single listener calls the latest render's callback; input fields keep their own keys.
    let key_action = use_mut_ref(|| Callback::<Control>::noop());
    *key_action.borrow_mut() = control.clone();
    {
        let key_action = key_action.clone();
        use_effect_with((), move |_| {
            let listener = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
                move |e: web_sys::KeyboardEvent| {
                    if e.ctrl_key() || e.meta_key() || e.alt_key() || e.repeat() {
                        return;
                    }
                    if let Some(element) = e
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    {
                        if element
                            .closest("input, textarea, select, [contenteditable=true]")
                            .ok()
                            .flatten()
                            .is_some()
                        {
                            return;
                        }
                        if e.key() == "Enter"
                            && element
                                .closest("button, summary, [role=button]")
                                .ok()
                                .flatten()
                                .is_some()
                        {
                            return;
                        }
                    }
                    let action = match e.key().to_lowercase().as_str() {
                        "q" => Control::Left,
                        "r" if e.shift_key() => Control::Left,
                        "r" => Control::Right,
                        "f" => Control::Flip,
                        "escape" => Control::Cancel,
                        "enter" => Control::Confirm,
                        "arrowleft" => Control::Move(-1, 0),
                        "arrowright" => Control::Move(1, 0),
                        "arrowup" => Control::Move(0, -1),
                        "arrowdown" => Control::Move(0, 1),
                        _ => return,
                    };
                    e.prevent_default();
                    key_action.borrow().emit(action);
                },
            );
            let window = web_sys::window().unwrap();
            let _ = window
                .add_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref());
            move || {
                let _ = window.remove_event_listener_with_callback(
                    "keydown",
                    listener.as_ref().unchecked_ref(),
                );
            }
        });
    }
    let Some(state) = props.snapshot.as_ref() else {
        crate::new_game::authority::publish(None, Callback::noop());
        return html! {<main class="game-table"><div key="hud" class="game-hud"/><div key="scene" class="quilt-scene"><BattleCanvas key="canvas"/><div key="overlay" class="battle-overlay"><div class="battle-welcome"><h2>{"一起缝制这盘时光"}</h2><p>{"创建好友房，分享房间码，双方准备后开始。"}</p><p>{"5 枚起始纽扣 · 时间落后者行动 · 首个 7×7 得 7 分"}</p></div></div></div></main>};
    };
    let Some((me, opponent)) = state.perspective(&props.user_id) else {
        return html! {<main class="game-table">{"正在恢复玩家座位…"}</main>};
    };
    let own = me.seat();
    let special = special_for(state, own);
    let active = props.connected && props.synced && !props.busy && state.input_actor() == Some(own);
    let action = intent(state, own, *draft);
    let validity = action.map(|a| state.apply_action(&props.user_id, a));
    let valid = validity.as_ref().is_some_and(Result::is_ok);
    crate::new_game::authority::publish(
        Some(crate::new_game::authority::SceneInput {
            snapshot: (**state).clone(),
            own,
            version: props.version,
            draft: *draft,
            active,
            valid,
        }),
        control.reform(|e| match e {
            crate::new_game::authority::SceneEvent::Select(id) => Control::Select(id),
            crate::new_game::authority::SceneEvent::Anchor(p) => Control::Anchor(p),
            crate::new_game::authority::SceneEvent::PlaceAt(p) => Control::PlaceAt(p),
        }),
    );
    let selected = draft.patch.and_then(patch);
    // Candidates can be inspected even when they cannot currently be purchased.
    let selection_problem = selected.and_then(|p| {
        let required = u32::from(p.button_cost);
        if required > me.buttons() {
            Some(format!(
                "纽扣不足：需要 {required}，当前 {}，还差 {}。可预览，不能购买。",
                me.buttons(),
                required - me.buttons()
            ))
        } else if !placeable.contains(&p.id) {
            Some("图版没有可用落点。可预览，不能购买。".into())
        } else {
            None
        }
    });
    let status = if !props.connected {
        "连接中断，正在恢复对局".into()
    } else if !props.synced {
        "正在同步已保存的对局".into()
    } else if state.lifecycle() == Lifecycle::Paused {
        "对局暂停，等待双方恢复连接".into()
    } else if state.result().is_some() {
        "对局已结束".into()
    } else if special.is_some() {
        format!("轮到你 · 放置 {} 号特殊拼布", special.unwrap())
    } else if state.input_actor() == Some(own) {
        "轮到你 · 选一块拼布，或前进".into()
    } else {
        "对手正在行动".into()
    };
    let feedback = if props.busy {
        "正在确认操作，请稍候。".to_string()
    } else if !active {
        status.clone()
    } else if let Some(message) = selection_problem.as_ref() {
        message.clone()
    } else if let Some(Err(e)) = validity.as_ref() {
        problem(*e).into()
    } else if valid {
        "落点合法，左键点击图版即可放置；也可按 Enter 或点击确认。".into()
    } else if special.is_some() {
        "免费放置 1×1；不耗时、无收入。左键点击自己的图版即可放置。".into()
    } else if selected.is_some() {
        "移动鼠标预览，左键点击自己的图版放置；方向键微调。".into()
    } else {
        "绿色框是接下来的候选。选块后可旋转、翻面，左键点击自己的图版放置。".into()
    };
    let advance_steps = (opponent
        .time_position()
        .saturating_add(1)
        .min(CUSTOM_V1.track.end))
    .saturating_sub(me.time_position());
    let advance_income = CUSTOM_V1
        .track
        .income_positions
        .iter()
        .filter(|&&p| p > me.time_position() && p <= me.time_position() + advance_steps)
        .count() as u32
        * me.income();
    let click = |c| {
        let control = control.clone();
        Callback::from(move |_| control.emit(c))
    };
    let leave = {
        let emit = props.on_leave.clone();
        Callback::from(move |_| emit.emit(()))
    };
    let cancel_with_mouse = {
        let control = control.clone();
        Callback::from(move |event: MouseEvent| {
            event.prevent_default();
            control.emit(Control::Cancel);
        })
    };
    html! {
        <main class="game-table" oncontextmenu={cancel_with_mouse} data-testid="game-table" data-game-id={state.game_id().to_string()} data-version={props.version.to_string()} data-state-checksum={(*state_checksum).clone()} data-actor={state.input_actor().map(|s|s.index().to_string()).unwrap_or_default()} data-own-seat={own.index().to_string()} data-phase={format!("{:?}",state.action_phase())}>
            <div key="hud" class="game-hud">
                <GameHud player={opponent.clone()} name={props.names[opponent.seat().index()].clone()} mine={false} bonus={state.bonus().owner()==Some(opponent.seat())}/>
                <div class="turn-label" role="status"><strong>{&status}</strong><small>{format!("剩余 {} 块",state.supply().remaining_count())}</small><div class="candidate-shortcuts" aria-label="当前候选拼布">
                    {for state.supply().candidates().iter().map(|&id|{let p=patch(id).unwrap();let affordable=u32::from(p.button_cost)<=me.buttons();let fits=placeable.contains(&id);let reason=if !affordable{"可预览，纽扣不足"}else if !fits{"可预览，无可用落点"}else{"可选择"};html!{<button data-patch={id.0.to_string()} data-candidate="true" title={format!("拼布 {}：费用 {}，时间 {}，收入 {}，{}",id.0,p.button_cost,p.time_cost,p.income,reason)} disabled={!active||special.is_some()} onclick={click(Control::Select(id))}>{format!("#{}{}",id.0,if !affordable{" 缺钮"}else if !fits{" 无位"}else{""})}</button>}})}
                </div></div>
                <GameHud player={me.clone()} name={props.names[own.index()].clone()} mine={true} bonus={state.bonus().owner()==Some(own)}/>
            </div>
            <div key="scene" class="quilt-scene">
                <BattleCanvas key="canvas"/>
                <div key="overlay" class="battle-overlay">
                if state.result().is_some() && *show_result {
                    <GameResultPanel snapshot={state.clone()} own={own} on_leave={leave.clone()} on_close={let show_result=show_result.clone();Callback::from(move |_|show_result.set(false))} disabled={props.busy || !props.connected}/>
                }
                </div>
            </div>
            <div key="toolbar" class="placement-toolbar" aria-busy={props.busy.to_string()} data-selected={draft.patch.map(|p|p.0.to_string()).unwrap_or_default()} data-anchor={draft.anchor.map(|p|format!("{},{}",p.coordinates().0,p.coordinates().1)).unwrap_or_default()} data-rotation={draft.orientation.quarter_turns().to_string()} data-flipped={draft.orientation.flipped().to_string()} data-valid={valid.to_string()}>
                <div class="placement-description"><strong>{if let Some(p)=selected {format!("拼布 #{} · 费用 {} · 时间 {} · 收入 +{}",p.id.0,p.button_cost,p.time_cost,p.income)}else if special.is_some(){"特殊拼布 · 1×1".into()}else{"选择拼布 / 前进".into()}}</strong><p class={classes!((selection_problem.is_some() || validity.as_ref().is_some_and(Result::is_err)).then_some("invalid-hint"))}>{feedback}</p></div>
                <div class="placement-actions">
                    <button onclick={click(Control::Left)} disabled={!active||special.is_some()||selected.is_none()} title="Q / Shift+R">{"↶ 左转"}</button>
                    <button onclick={click(Control::Right)} disabled={!active||special.is_some()||selected.is_none()} title="R">{"↷ 右转"}</button>
                    <button onclick={click(Control::Flip)} disabled={!active||special.is_some()||selected.is_none()} title="F">{"⇄ 翻面"}</button>
                    <button onclick={click(Control::Cancel)} disabled={!active || (selected.is_none()&&draft.anchor.is_none())} title="Esc / 右键">{"取消"}</button>
                    <button class="confirm-placement" onclick={click(Control::Confirm)} disabled={!active||!valid} title="左键点击自己的图版 / Enter">{"确认放置"}</button>
                    <button class="advance-action" onclick={click(Control::Advance)} disabled={!active||special.is_some()} title={format!("前进 {} 格，步数奖励 {}，途中收入 {}",advance_steps,advance_steps,advance_income)}>{format!("前进 {} 格 · +{} 纽扣",advance_steps,u32::from(advance_steps)+advance_income)}</button>
                    if state.result().is_some(){<button onclick={let show_result=show_result.clone();Callback::from(move |_|show_result.set(true))}>{"查看结果"}</button>}
                </div>
                <small class="shortcut-note">{"Q / Shift+R 左转 · R 右转 · F 翻面 · Esc / 右键取消 · 左键图版 / Enter 放置　外围：费用 / 时间 / 收入"}</small>
            </div>
            <TimeTrack key="track" snapshot={state.clone()} own={own} on_note={let marker_note=marker_note.clone();Callback::from(move |s|marker_note.set(s))}/>
            <div key="note" class="track-note">{if marker_note.is_empty(){"● 纽扣收入点　◆ 特殊拼布　同格时换另一名玩家行动"}else{&*marker_note}}</div>
        </main>
    }
}

#[function_component(BattleCanvas)]
fn battle_canvas() -> Html {
    let canvas = use_node_ref();
    {
        let canvas = canvas.clone();
        use_effect_with((), move |_| {
            let canvas = canvas
                .cast::<web_sys::HtmlCanvasElement>()
                .expect("battle canvas");
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(e) = crate::game::run_game(canvas).await {
                    web_sys::console::error_1(&e.into());
                }
            });
            || ()
        });
    }
    html! {<canvas ref={canvas} id="game-canvas" tabindex="0" aria-label="Bevy 拼布对局：左侧对手图版，右侧自己的图版"/>}
}

fn has_space(player: &PlayerState, id: PatchId) -> bool {
    distinct_orientations(id)
        .unwrap_or_default()
        .iter()
        .any(|(o, _)| {
            (0..9).any(|y| {
                (0..9).any(|x| {
                    player
                        .board()
                        .preview_placement(
                            PieceId::Normal(id),
                            BoardPosition::new(x, y).unwrap(),
                            *o,
                        )
                        .is_ok()
                })
            })
        })
}
#[derive(Properties, PartialEq)]
struct HudProps {
    player: PlayerState,
    name: String,
    mine: bool,
    bonus: bool,
}
#[function_component(GameHud)]
fn hud(p: &HudProps) -> Html {
    html! {<section class={classes!("player-hud",p.mine.then_some("my-hud"))} data-owner={p.player.user_id().to_string()}><strong>{format!("{} · {}",if p.mine{"你"}else{"对手"},p.name)}</strong><div><span class="button-balance">{format!("◉ {}",p.player.buttons())}</span><span title="图版上所有拼布的收入之和；每经过一个纽扣收入点，加到纽扣余额中">{format!("每次收入 +{}",p.player.income())}</span><span>{format!("时间 {}",p.player.time_position())}</span><span class={classes!("bonus-badge",p.bonus.then_some("awarded"))}>{if p.bonus{"7×7 +7分"}else{"7×7 未得"}}</span></div></section>}
}

#[derive(Properties, PartialEq)]
struct TrackProps {
    snapshot: Rc<GameSnapshot>,
    own: Seat,
    on_note: Callback<String>,
}
#[function_component(TimeTrack)]
fn track(p: &TrackProps) -> Html {
    let explain = |pos: u8| {
        if let Some(s) = p
            .snapshot
            .special_patches()
            .iter()
            .find(|s| s.track_position() == pos)
        {
            let detail = match s.status() {
                SpecialPatchStatus::Available => "尚未领取".into(),
                SpecialPatchStatus::Pending { owner } => {
                    format!("{}已领取，待放", if owner == p.own { "你" } else { "对手" })
                }
                SpecialPatchStatus::Placed { owner, .. } => {
                    format!("{}已放置", if owner == p.own { "你" } else { "对手" })
                }
                SpecialPatchStatus::Discarded { .. } => "图版已满，已自动弃置".into(),
            };
            format!("第 {pos} 格特殊拼布：{detail}")
        } else if CUSTOM_V1.track.income_positions.contains(&pos) {
            format!(
                "第 {pos} 格：经过时将图版收入之和加到纽扣余额（你当前 +{}，对手当前 +{}）",
                p.snapshot.player(p.own).income(),
                p.snapshot.player(p.own.other()).income()
            )
        } else {
            format!("时间图版第 {pos} 格，终点 53")
        }
    };
    html! {<svg class="time-track" viewBox="0 0 1200 106" role="group" aria-label="完整时间图版：0 到 53，双方独立通道">
        <text x="6" y="23" class="track-label">{"对手"}</text><text x="6" y="51" class="track-label">{"你"}</text>
        {for (0..=53u8).map(|pos|{let x=76+i32::from(pos)*20;let note=explain(pos);let callback={let emit=p.on_note.clone();let note=note.clone();Callback::from(move |_|emit.emit(note.clone()))};let special=p.snapshot.special_patches().iter().find(|s|s.track_position()==pos);let available=special.is_some_and(|s|s.status()==SpecialPatchStatus::Available);html!{<g data-track-position={pos.to_string()} onclick={callback}><title>{note}</title><rect x={(x-8).to_string()} y="10" width="17" height="46" rx="3" class="time-cell"/>
            if pos%5==0||pos==53{<text x={x.to_string()} y="73" text-anchor="middle" class="track-number">{pos}</text>}
            if CUSTOM_V1.track.income_positions.contains(&pos){<text x={x.to_string()} y="94" text-anchor="middle" class="income-marker">{"●"}</text>}
            if special.is_some(){<text x={x.to_string()} y="94" text-anchor="middle" class={classes!("special-marker",(!available).then_some("claimed"))}>{if available{"◆"}else{"◇"}}</text>}
        </g>}})}
        {for [p.own.other(),p.own].iter().enumerate().map(|(row,&seat)|html!{<TimeToken owner={seat} position={p.snapshot.player(seat).time_position()} row={row}/>})}
    </svg>}
}
#[derive(Properties, PartialEq)]
struct TokenProps {
    owner: Seat,
    position: u8,
    row: usize,
}
#[function_component(TimeToken)]
fn token(p: &TokenProps) -> Html {
    html! {<g class="time-token" data-owner-seat={p.owner.index().to_string()} data-position={p.position.to_string()} transform={format!("translate({} {})",76+u32::from(p.position)*20,22+p.row*28)}><circle r="10" fill={seat_color(p.owner)}/><text y="4" text-anchor="middle">{p.position}</text></g>}
}
#[derive(Properties, PartialEq)]
struct ResultProps {
    snapshot: Rc<GameSnapshot>,
    own: Seat,
    on_leave: Callback<MouseEvent>,
    on_close: Callback<MouseEvent>,
    disabled: bool,
}
#[function_component(GameResultPanel)]
fn result_panel(p: &ResultProps) -> Html {
    let result = p.snapshot.result().unwrap();
    let title = match result.outcome {
        Outcome::Won { winner } if winner == p.own => "你赢了！",
        Outcome::Won { .. } => "对手获胜",
        Outcome::Draw => "平局",
        Outcome::Abandoned => "对局已结束，无胜者",
    };
    html! {<section class="game-result" aria-label="对局结果"><h2>{title}</h2><p>{match result.reason{game_core::state::ResultReason::Scored=>"双方到达终点，按纽扣与奖励计分。",game_core::state::ResultReason::Forfeit{..}=>"对局因认输或恢复超时结束。",_=>"双方离线超过保留期限。"}}</p><table><thead><tr><th>{"玩家"}</th><th>{"纽扣"}</th><th>{"7×7"}</th><th>{"总分"}</th></tr></thead><tbody>{for [p.own.other(),p.own].iter().map(|&seat|{let score=&result.scores[seat.index()];html!{<tr><th>{if seat==p.own{"你"}else{"对手"}}</th><td>{score.buttons}</td><td>{score.bonus_points}</td><td><strong>{score.total}</strong></td></tr>}})}</tbody></table><small>{"空格不扣分。返回大厅后可创建新的好友房，再开一局。"}</small><div><button onclick={p.on_close.clone()}>{"查看图版"}</button><button disabled={p.disabled} onclick={p.on_leave.clone()}>{"返回大厅"}</button></div></section>}
}
