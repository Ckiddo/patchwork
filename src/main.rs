pub mod app;
pub mod game;
pub mod new_game;
pub mod ui;
pub mod login;

fn main() {
    yew::Renderer::<app::App>::new().render();
}