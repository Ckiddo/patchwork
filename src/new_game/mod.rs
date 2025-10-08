pub mod game_state;
use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};

use crate::new_game::game_state::{hello_ui, init_game_resource, load_hello_ui_res, GameState};

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
        // 初始化游戏资源
        app.add_systems(OnEnter(GameState::InGame),init_game_resource);

    }
}
