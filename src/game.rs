
use bevy::prelude::*;
use web_sys::HtmlCanvasElement;

use crate::new_game::NewGamePlug;

pub async fn run_game(canvas: HtmlCanvasElement, token: String) -> Result<(), String>{
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
        .add_systems(Update, update_camera_projection)
        .run();
    Ok(())
}

pub const WIDTH_BASE: f32 = 100.0;

fn setup_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
}

pub const WIDTH: f32 = 1920.0;
pub const HEIGHT: f32 = 1080.0;
fn update_camera_projection(mut camera:Single<&mut Projection, With<Camera2d>>){
    match &mut (**camera){
        Projection::Orthographic(o) => {
            o.area = Rect::from_center_size(Vec2::ZERO, vec2(WIDTH, HEIGHT));
            o.scaling_mode = bevy::camera::ScalingMode::Fixed { width: WIDTH, height: HEIGHT };
        },
        _ => {
            warn!("not desired projection: {:?}", &(**camera));
        }
    }
}
