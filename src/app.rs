use serde::{Deserialize, Serialize};
use util_lib::UserIdentity;
use wasm_bindgen::prelude::*;
use web_sys::{Storage, window};
use yew::{Callback, Html, function_component, html, use_node_ref, use_state};

pub fn base_url() -> &'static str {
    let base_url = if cfg!(debug_assertions) {
        "http://127.0.0.1:5380"
    } else {
        "http://www.ckiddo.com:5380"
    };
    base_url
}

const JWT_STORAGE_KEY: &str = "game_jwt_token";
const API_BASE_URL: &str = "http://localhost:5380/api";

#[derive(Clone, Serialize, Deserialize)]
struct JwtRsp {
    pub jwt: String,
    pub identity: UserIdentity,
}

#[function_component(App)]
pub fn app() -> Html {
    let canvas_ref = use_node_ref();
    let game_started = use_state(|| false);
    let jwt_token = use_state(|| Option::<String>::None);
    let user_info = use_state(|| Option::<(String, String)>::None); // (user_id, nickname)
    let is_loading = use_state(|| true);
    let error_message = use_state(|| Option::<String>::None);

    // 初始化JWT身份
    {
        let jwt_token = jwt_token.clone();
        let user_info = user_info.clone();
        let is_loading = is_loading.clone();
        let error_message = error_message.clone();

        yew::use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match initialize_jwt().await {
                    Ok((token, user_id, nickname)) => {
                        jwt_token.set(Some(token));
                        user_info.set(Some((user_id, nickname)));
                        error_message.set(None);
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

    // 开始游戏
    let start_game = {
        let canvas_ref = canvas_ref.clone();
        let game_started = game_started.clone();
        let jwt_token = jwt_token.clone();
        let error_message = error_message.clone();

        Callback::from(move |_| {
            if *game_started {
                return;
            }

            let token = match (*jwt_token).as_ref() {
                Some(t) => t.clone(),
                None => {
                    error_message.set(Some("JWT令牌未初始化".to_string()));
                    return;
                }
            };

            let canvas = canvas_ref.cast::<web_sys::HtmlCanvasElement>().unwrap();
            game_started.set(true);

            wasm_bindgen_futures::spawn_local(async move {
                if let Err(e) = crate::game::run_game(canvas, token).await {
                    web_sys::console::error_1(&format!("Game error: {}", e).into());
                }
            });
        })
    };

    html!(
        <div class="app-container">
            <header>
                <h1>{ "Bevy WebSocket Game" }</h1>
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
                        </div>
                    )
                } else if let Some((user_id, nickname)) = (*user_info).as_ref() {
                    html!(
                        <>
                            <div class="user-info">
                                <div class="user-badge">
                                    <span class="nickname">{ nickname }</span>
                                    <span class="user-id">{ format!("#{}", &user_id[..8]) }</span>
                                </div>
                                <div class="jwt-status">
                                    { "✓ 已认证" }
                                </div>
                            </div>

                            <main>
                                <canvas ref={canvas_ref} id="game-canvas"></canvas>

                                {
                                    if !*game_started {
                                        html!(
                                            <div class="game-overlay">
                                                <button class="start-button" onclick={start_game}>
                                                    { "▶️ 开始游戏" }
                                                </button>
                                                <p class="hint">{ "游戏将通过WebSocket连接到服务器" }</p>
                                            </div>
                                        )
                                    } else {
                                        html!(<></>)
                                    }
                                }
                            </main>
                        </>
                    )
                } else {
                    html!(<></>)
                }
            }

            <style>
                {r#"
                    * {
                        margin: 0;
                        padding: 0;
                        box-sizing: border-box;
                    }

                    body {
                        font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
                        background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
                        min-height: 100vh;
                    }

                    .app-container {
                        max-width: 1920px;
                        margin: 0 auto;
                        padding: 20px;
                    }

                    header {
                        text-align: center;
                        color: white;
                        margin-bottom: 30px;
                    }

                    header h1 {
                        font-size: 2.5rem;
                        text-shadow: 2px 2px 4px rgba(0,0,0,0.2);
                    }

                    .loading-screen, .error-screen {
                        background: white;
                        border-radius: 20px;
                        padding: 60px 40px;
                        text-align: center;
                        box-shadow: 0 20px 60px rgba(0,0,0,0.3);
                    }

                    .spinner {
                        width: 50px;
                        height: 50px;
                        border: 4px solid #f3f3f3;
                        border-top: 4px solid #667eea;
                        border-radius: 50%;
                        animation: spin 1s linear infinite;
                        margin: 0 auto 20px;
                    }

                    @keyframes spin {
                        0% { transform: rotate(0deg); }
                        100% { transform: rotate(360deg); }
                    }

                    .error-screen h2 {
                        color: #e53e3e;
                        margin-bottom: 15px;
                    }

                    .error-screen button {
                        margin-top: 20px;
                        padding: 12px 30px;
                        background: #667eea;
                        color: white;
                        border: none;
                        border-radius: 8px;
                        font-size: 1rem;
                        cursor: pointer;
                        transition: transform 0.2s;
                    }

                    .error-screen button:hover {
                        transform: scale(1.05);
                    }

                    .user-info {
                        background: rgba(255, 255, 255, 0.95);
                        border-radius: 15px;
                        padding: 20px;
                        margin-bottom: 30px;
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        box-shadow: 0 10px 30px rgba(0,0,0,0.2);
                    }

                    .user-badge {
                        display: flex;
                        flex-direction: column;
                        gap: 5px;
                    }

                    .nickname {
                        font-size: 1.3rem;
                        font-weight: bold;
                        color: #2d3748;
                    }

                    .user-id {
                        font-size: 0.9rem;
                        color: #718096;
                        font-family: 'Courier New', monospace;
                    }

                    .jwt-status {
                        padding: 8px 16px;
                        background: #48bb78;
                        color: white;
                        border-radius: 20px;
                        font-size: 0.9rem;
                        font-weight: 600;
                    }

                    main {
                        position: relative;
                        background: rgba(0, 0, 0, 0.8);
                        border-radius: 20px;
                        overflow: hidden;
                        box-shadow: 0 20px 60px rgba(0,0,0,0.4);
                    }

                    #game-canvas {
                        display: block;
                        width: 100%;
                        height: 1080px;
                        background: #1a202c;
                    }

                    .game-overlay {
                        position: absolute;
                        top: 0;
                        left: 0;
                        width: 100%;
                        height: 100%;
                        display: flex;
                        flex-direction: column;
                        justify-content: center;
                        align-items: center;
                        background: rgba(0, 0, 0, 0.7);
                        backdrop-filter: blur(5px);
                    }

                    .start-button {
                        padding: 20px 50px;
                        font-size: 1.5rem;
                        background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
                        color: white;
                        border: none;
                        border-radius: 50px;
                        cursor: pointer;
                        box-shadow: 0 10px 30px rgba(102, 126, 234, 0.4);
                        transition: all 0.3s ease;
                        font-weight: bold;
                    }

                    .start-button:hover {
                        transform: translateY(-3px) scale(1.05);
                        box-shadow: 0 15px 40px rgba(102, 126, 234, 0.6);
                    }

                    .start-button:active {
                        transform: translateY(-1px) scale(1.02);
                    }

                    .hint {
                        margin-top: 20px;
                        color: rgba(255, 255, 255, 0.7);
                        font-size: 0.9rem;
                    }
                "#}
            </style>
        </div>
    )
}

// 初始化JWT：从localStorage读取，如果没有则向服务器请求
async fn initialize_jwt() -> Result<(String, String, String), String> {
    let storage = get_local_storage()?;

    // 1. 尝试从localStorage读取
    if let Ok(Some(cached_token)) = storage.get_item(JWT_STORAGE_KEY) {
        web_sys::console::log_1(&"Found cached JWT token".into());

        // 验证token是否有效（可选：向服务器验证）
        // if let Ok(info) = decode_jwt(&cached_token) {
        //     return Ok((cached_token, info.0, info.1));
        // }
        if let Ok(info) = request_verify(&cached_token).await {
            return Ok((cached_token, info.identity.user_id, info.identity.nickname));
        }

        web_sys::console::log_1(&"Cached token invalid, requesting new one".into());
    }

    // 2. 向服务器请求新的JWT
    let jwt_response = request_new_jwt().await?;

    // 3. 保存到localStorage
    storage
        .set_item(JWT_STORAGE_KEY, &jwt_response.jwt)
        .map_err(|_| "保存JWT到localStorage失败".to_string())?;

    Ok((
        jwt_response.jwt,
        jwt_response.identity.user_id,
        jwt_response.identity.nickname,
    ))
}

#[derive(Deserialize)]
pub struct VerifyRsp {
    pub identity: UserIdentity,
}
async fn request_verify(jwt: &String) -> Result<VerifyRsp, String> {
    let window = window().ok_or("无法获取window对象")?;

    let url = format!("{}/auth/verify", API_BASE_URL);

    let opts = web_sys::RequestInit::new();
    opts.set_method("POST");
    opts.set_mode(web_sys::RequestMode::Cors);

    let request =
        web_sys::Request::new_with_str_and_init(&url, &opts).map_err(|_| "创建请求失败")?;

    request
        .headers()
        .set("Content-Type", "application/json")
        .map_err(|_| "设置请求头失败")?;

    request
        .headers()
        .set("Authorization", &format!("Bearer {}", jwt))
        .map_err(|_| "设置请求头失败")?;

    let resp_value = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| "网络请求失败")?;

    let resp: web_sys::Response = resp_value.dyn_into().map_err(|_| "响应类型转换失败")?;

    if !resp.ok() {
        return Err(format!("服务器返回错误: {}", resp.status()));
    }

    let json = wasm_bindgen_futures::JsFuture::from(resp.json().map_err(|_| "解析JSON失败")?)
        .await
        .map_err(|_| "读取响应体失败")?;

    // web_sys::console::log_1(&json);

    serde_wasm_bindgen::from_value(json).map_err(|e| format!("解析JWT响应失败: {:?}", e))
}

// 向服务器请求新的JWT
async fn request_new_jwt() -> Result<JwtRsp, String> {
    let window = window().ok_or("无法获取window对象")?;

    let url = format!("{}/auth/create", API_BASE_URL);

    let opts = web_sys::RequestInit::new();
    opts.set_method("POST");
    opts.set_mode(web_sys::RequestMode::Cors);

    let request =
        web_sys::Request::new_with_str_and_init(&url, &opts).map_err(|_| "创建请求失败")?;

    request
        .headers()
        .set("Content-Type", "application/json")
        .map_err(|_| "设置请求头失败")?;

    let resp_value = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| "网络请求失败")?;

    let resp: web_sys::Response = resp_value.dyn_into().map_err(|_| "响应类型转换失败")?;

    if !resp.ok() {
        return Err(format!("服务器返回错误: {}", resp.status()));
    }

    let json = wasm_bindgen_futures::JsFuture::from(resp.json().map_err(|_| "解析JSON失败")?)
        .await
        .map_err(|_| "读取响应体失败")?;

    web_sys::console::log_1(&json);

    serde_wasm_bindgen::from_value(json).map_err(|e| format!("解析JWT响应失败: {:?}", e))
}

// 获取localStorage
fn get_local_storage() -> Result<Storage, String> {
    window()
        .ok_or("无法获取window对象")?
        .local_storage()
        .map_err(|_| "无法访问localStorage")?
        .ok_or("localStorage不可用".to_string())
}
