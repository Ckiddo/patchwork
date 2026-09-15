pub mod authority;
pub mod chessboard;
pub mod event;
pub mod game_state;
pub mod patches;
use crate::game::{HEIGHT, WIDTH};
use bevy::prelude::*;
use bevy_egui::egui::vec2;

pub struct NewGamePlug;

impl Plugin for NewGamePlug {
    fn build(&self, app: &mut App) {
        // Keep the existing Bevy scene entry point. Server snapshots now own the game;
        // the old HelloUI/local BoardGame mutation systems are no longer registered.
        app.add_plugins(authority::AuthorityPlugin);
    }
}

pub fn mid_pos(row: i32, col: i32, square_size: f32) -> bevy_egui::egui::Vec2 {
    // 计算方块中心位置（考虑到坐标从 (0,0) 到 (1920, 1080)）
    let x = col as f32 * square_size + square_size / 2.0 - WIDTH / 2.0;
    let y = row as f32 * square_size + square_size / 2.0 - HEIGHT / 2.0;
    vec2(x, y)
}

pub fn generate_color(row: i32, col: i32, rows: i32, cols: i32) -> Color {
    let index = (row * cols + col) as f32;
    let total = (rows * cols) as f32;
    let hue = (index / total) * 360.0;
    Color::hsl(hue, 0.8, 0.6)
}
