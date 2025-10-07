// use yew::prelude::*;
pub mod app;
pub mod game;
pub mod new_game;
pub mod ui;

fn main() {
    // yew::Renderer::<App>::new().render();
    yew::Renderer::<app::App>::new().render();
}