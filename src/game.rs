use bevy::prelude::*;
use web_sys::HtmlCanvasElement;

use crate::new_game::NewGamePlug;

pub async fn run_game(canvas: HtmlCanvasElement) {
    App::new()
        // .register_asset_source("embedded", AssetSourceBuilder::platform_default("asset", None))
        .add_plugins((
            bevy_embedded_assets::EmbeddedAssetPlugin::default(),
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    canvas: Some(format!("#{}", canvas.id())),
                    fit_canvas_to_parent: true,
                    ..default()
                }),
                ..default()
            }), // 如果你要自己手动放置一个assets文件夹到wasm同级用来拿图片资源，就需要
                // .set(AssetPlugin {
                //     meta_check: AssetMetaCheck::Never,
                //     ..default()
                // }),
        ))
        .add_plugins(NewGamePlug)
        .add_systems(Startup, setup_camera)
        .run();
}

pub const WIDTH_BASE: f32 = 100.0;

fn setup_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
}
