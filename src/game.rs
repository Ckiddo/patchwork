use bevy::prelude::*;
use web_sys::HtmlCanvasElement;

use crate::new_game::NewGamePlug;

pub async fn run_game(canvas: HtmlCanvasElement) -> Result<(), String> {
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
            }),
        ))
        .add_plugins(NewGamePlug)
        .add_systems(Startup, setup_camera)
        .run();
    Ok(())
}

pub const WIDTH_BASE: f32 = 100.0;

fn setup_camera(mut commands: Commands) {
    // Keep the entire board visible as the browser and room sidebar resize.
    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: bevy::camera::ScalingMode::AutoMin {
                min_width: WIDTH,
                min_height: HEIGHT,
            },
            ..OrthographicProjection::default_2d()
        }),
    ));
}

pub const WIDTH: f32 = 1920.0;
pub const HEIGHT: f32 = 1080.0;
