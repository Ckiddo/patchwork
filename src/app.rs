use wasm_bindgen::prelude::*;
use web_sys::window;
use yew::{Callback, Html, function_component, html, use_state};

pub fn jwt_base_url() -> &'static str {
    match option_env!("PATCHWORK_API_BASE") {
        Some(url) => url,
        None if cfg!(debug_assertions) => "http://127.0.0.1:8000/api",
        None => "",
    }
}

#[function_component(App)]
pub fn app() -> Html {
    let jwt_token = use_state(|| Option::<String>::None);
    let user_info = use_state(|| Option::<(String, String)>::None); // (user_id, nickname)
    let is_loading = use_state(|| true);
    let error_message = use_state(|| Option::<String>::None);
    let connected = use_state(|| false);
    let connection_epoch = use_state(|| 0u32);
    let connection_status = use_state(String::new);

    // 初始化JWT身份
    {
        let jwt_token = jwt_token.clone();
        let user_info = user_info.clone();
        let is_loading = is_loading.clone();
        let error_message = error_message.clone();
        let connected = connected.clone();
        let connection_epoch = connection_epoch.clone();

        yew::use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match crate::browser_session::initialize(jwt_base_url()).await {
                    Ok((token, user_id, nickname)) => {
                        jwt_token.set(Some(token));
                        user_info.set(Some((user_id, nickname)));
                        error_message.set(None);
                        connected.set(true);
                        connection_epoch.set(1);
                    }
                    Err(e) => {
                        error_message.set(Some(format!("获取身份失败: {}", e)));
                        web_sys::console::error_1(&format!("JWT init failed: {}", e).into());
                    }
                }
                is_loading.set(false);
            });

            || ()
        });
    }

    {
        let connected = connected.clone();
        let epoch = connection_epoch.clone();
        let status = connection_status.clone();
        let jwt = jwt_token.clone();
        yew::use_effect_with((), move |_| {
            let running = std::rc::Rc::new(std::cell::Cell::new(false));
            let cancelled = std::rc::Rc::new(std::cell::Cell::new(false));
            let stop = cancelled.clone();
            let counter = std::rc::Rc::new(std::cell::Cell::new(1u32));
            let callback =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                    connected.set(false);
                    let retry = event
                        .dyn_ref::<web_sys::CustomEvent>()
                        .and_then(|e| e.detail().as_bool())
                        .unwrap_or(false);
                    if !retry {
                        cancelled.set(true);
                        status.set("连接已被另一页面接管或已结束。需要时可重新加载此页。".into());
                        return;
                    }
                    if running.replace(true) {
                        return;
                    }
                    let connected = connected.clone();
                    let epoch = epoch.clone();
                    let status = status.clone();
                    let jwt = jwt.clone();
                    let running = running.clone();
                    let cancelled = cancelled.clone();
                    let counter = counter.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let mut attempt = 0;
                        while !cancelled.get() {
                            status.set("连接中断，正在恢复原身份与房间；操作记录已保留。".into());
                            crate::browser_session::reconnect_delay(attempt).await;
                            if cancelled.get() {
                                break;
                            }
                            match crate::browser_session::initialize(jwt_base_url()).await {
                                Ok((token, _, _)) => {
                                    jwt.set(Some(token));
                                    connected.set(true);
                                    counter.set(counter.get().wrapping_add(1));
                                    epoch.set(counter.get());
                                    status.set(String::new());
                                    break;
                                }
                                Err(_) => {
                                    attempt = attempt.saturating_add(1);
                                }
                            }
                        }
                        running.set(false);
                    });
                });
            if let Some(w) = window() {
                let _ = w.add_event_listener_with_callback(
                    "patchwork-session-ended",
                    callback.as_ref().unchecked_ref(),
                );
            }
            move || {
                stop.set(true);
                if let Some(w) = window() {
                    let _ = w.remove_event_listener_with_callback(
                        "patchwork-session-ended",
                        callback.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }

    html!(
        <div class="app-container">
            <header class="app-header">
                <h1>{ "Patchwork" }</h1>
                if let Some((user_id, nickname)) = (*user_info).as_ref() {
                    <div class="user-info">
                        <span class="nickname">{ nickname }</span>
                        <span class="user-id">{ format!("#{}", user_id.chars().take(8).collect::<String>()) }</span>
                        <span class="jwt-status">{ "✓ 已认证" }</span>
                    </div>
                }
            </header>

            {
                if *is_loading {
                    html!(
                        <div class="loading-screen">
                            <div class="spinner"></div>
                            <p>{ "正在获取身份..." }</p>
                        </div>
                    )
                } else if let Some(error) = (*error_message).as_ref() {
                    html!(
                        <div class="error-screen">
                            <h2>{ "❌ 错误" }</h2>
                            <p>{ error }</p>
                            <button onclick={Callback::from(|_| {
                                window().unwrap().location().reload().unwrap();
                            })}>
                                { "重新加载" }
                            </button>
                            if option_env!("PATCHWORK_LOCAL_TEST") == Some("true") {
                                <p>{"临时测试库已重新创建时，可重置此页面的测试身份。此操作放弃旧测试身份。"}</p>
                                <button onclick={Callback::from(|_| {
                                    let w = window().unwrap();
                                    if let Ok(Some(storage)) = w.local_storage() {
                                        let _ = storage.remove_item(&format!("patchwork_session_v1:{}", jwt_base_url()));
                                        let _ = storage.remove_item("game_jwt_token");
                                        let _ = w.location().reload();
                                    }
                                })}>{"重置本地测试身份"}</button>
                            }
                        </div>
                    )
                } else if let Some((user_id, _)) = (*user_info).as_ref() {
                    html!(
                        <>
                            if !connection_status.is_empty() {<p class="connection-status" role="status">{&*connection_status}</p>}
                            <div class="game-workspace">
                            <crate::friend_rooms::FriendRooms user_id={user_id.clone()} connected={*connected} connection_epoch={*connection_epoch} />
                            </div>
                        </>
                    )
                } else {
                    html!(<></>)
                }
            }

            <style>{include_str!("layout.css")}</style>
        </div>
    )
}
