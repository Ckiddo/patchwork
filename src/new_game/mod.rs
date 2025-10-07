pub mod game_state;
use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};

use crate::new_game::game_state::{GameState, hello_ui, load_hello_ui_res};

pub struct NewGamePlug;

impl Plugin for NewGamePlug {
    fn build(&self, app: &mut App) {
        // 一个开始按钮
        // 用egui
        // 自定义图片
        //
        app.add_plugins(EguiPlugin::default());
        app.init_state::<GameState>();
        app.add_systems(Startup, load_hello_ui_res);
        app.add_systems(
            EguiPrimaryContextPass,
            hello_ui.run_if(in_state(GameState::HelloUI)),
        );
    }
}
