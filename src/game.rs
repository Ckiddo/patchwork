use std::path::Path;

use bevy::{
    asset::{
         AssetPath,
        io::{ AssetSourceId},
    },
    prelude::*,
};
use web_sys::HtmlCanvasElement;

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
        .add_systems(Startup, setup)
        .run();
}

const WIDTH_BASE: f32 = 100.0;
// const SHIP_PNG : &[u8] = include_bytes!("../assets/ship.png");
fn setup(mut commands: Commands, assert_server: Res<AssetServer>) {
    let ship_path = Path::new("ship.png");
    let ship_source = AssetSourceId::from("embedded");
    let asset_path = AssetPath::from_path(&ship_path).with_source(ship_source);
    // let ship = images.add(setup_)
    commands.spawn(Camera2d);

    commands.spawn((
        Sprite {
            image: assert_server.load(asset_path),
            custom_size: Some(vec2(WIDTH_BASE, WIDTH_BASE)),
            ..default()
        },
        Ship,
    ));
}

#[derive(Component)]
struct Ship;
