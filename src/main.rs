pub mod app;
pub mod browser_session;
pub mod friend_rooms;
pub mod game;
pub mod game_view;
pub mod new_game;
pub mod ui;

fn main() {
    yew::Renderer::<app::App>::new().render();
}
