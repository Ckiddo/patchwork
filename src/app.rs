use yew::{function_component, html, use_node_ref, use_state, Callback, Html};



#[function_component(App)]
pub fn app() -> Html{
    let canvas_ref = use_node_ref();
    let game_started = use_state(|| false);
    let start_game = {
        let canvas_ref = canvas_ref.clone();
        let game_started = game_started.clone();

        Callback::from(move |_|{
            if *game_started{
                return;
            }

            let canvas = canvas_ref.cast::<web_sys::HtmlCanvasElement>().unwrap();
            wasm_bindgen_futures::spawn_local(async move {
                crate::game::run_game(canvas).await;
            });

            game_started.set(true)
        })
    };

    html!(
        <div class="app-container">
            <header>
                <h1>{ "🎮 Yew + Bevy Game" }</h1>
            </header>

            <div class="game-wrapper">
                <canvas 
                    ref={canvas_ref}
                    id="bevy-canvas"
                    width="1920"
                    height="1080"
                />
            </div>

            <div class="controls">
                <button onclick={start_game} disabled={*game_started}>
                    { if *game_started { "🎮 Game Running" } else { "🚀 Start Game" } }
                </button>
            </div>

        </div>
    )
}