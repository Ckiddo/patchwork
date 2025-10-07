use bevy::prelude::*;
use bevy_egui::{
    EguiContexts, EguiTextureHandle, EguiUserTextures,
    egui::{self, Align2, Id, vec2},
};

use crate::{
    game::WIDTH_BASE,
    ui::{HelloUiTextures, get_asset_path, my_button},
};

// GameState

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, States)]
pub enum GameState {
    #[default]
    HelloUI,
    InGame,
}

pub fn load_hello_ui_res(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut egui_textures: ResMut<EguiUserTextures>,
) {
    let idle: Handle<Image> = asset_server.load(get_asset_path("boarder/boader_idle.png"));
    let idle_id = egui_textures.add_image(EguiTextureHandle::Strong(idle));

    let hover: Handle<Image> = asset_server.load(get_asset_path("boarder/boader_hover.png"));
    let hover_id = egui_textures.add_image(EguiTextureHandle::Strong(hover));

    let click: Handle<Image> = asset_server.load(get_asset_path("boarder/boader_click.png"));
    let click_id = egui_textures.add_image(EguiTextureHandle::Strong(click));
    let resource = HelloUiTextures {
        button_idle: idle_id,
        button_hover: hover_id,
        button_click: click_id,
    };
    commands.insert_resource(resource);
}

pub fn hello_ui(
    mut contexts: EguiContexts,
    button_res: Res<HelloUiTextures>,
    mut next_gamestate: ResMut<NextState<GameState>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::Area::new(Id::new("hello_ui"))
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let r = my_button(
                    ui,
                    "start game",
                    &button_res.get_textures(),
                    vec2(WIDTH_BASE, WIDTH_BASE / 2.0),
                );

                if r.clicked() {
                    next_gamestate.set(GameState::InGame);
                }
            });
        });

    Ok(())
}
