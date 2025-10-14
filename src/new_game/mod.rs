pub mod chessboard;
pub mod event;
pub mod game_state;
pub mod patches;
use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass, egui::vec2};

use crate::{
    game::{HEIGHT, WIDTH},
    new_game::{
        event::observe_patch_choose_event,
        game_state::{
            GameState, del_game_component, hello_ui, init_game_resource, load_hello_ui_res,
        },
    },
};

pub struct NewGamePlug;

impl Plugin for NewGamePlug {
    fn build(&self, app: &mut App) {
        // 一个开始按钮
        // 用egui
        // 自定义图片

        // egui
        app.add_plugins(EguiPlugin::default());

        // game state
        app.init_state::<GameState>();

        // ui按钮 ziyuan
        app.add_systems(Startup, load_hello_ui_res);

        // 开始界面的ui
        app.add_systems(
            EguiPrimaryContextPass,
            hello_ui.run_if(in_state(GameState::HelloUI)),
        );

        // 每新开一局就
        // 初始化后端游戏资源数据
        // 初始化前端交互标记资源
        app.add_systems(OnEnter(GameState::InGame), init_game_resource);

        // 删除游戏资源和compnent
        app.add_systems(OnExit(GameState::InGame), del_game_component);

        // 选中之后的事件
        app.add_observer(observe_patch_choose_event);
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
