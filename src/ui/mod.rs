use std::path::Path;

use bevy::{
    asset::{AssetPath, io::AssetSourceId},
    ecs::resource::Resource,
};
use bevy_egui::egui::{Color32, Response, Sense, TextureId, Ui, Vec2, pos2};

#[derive(Resource)]
pub struct HelloUiTextures {
    pub button_idle: TextureId,
    pub button_hover: TextureId,
    pub button_click: TextureId,
}
impl HelloUiTextures {
    pub fn get_textures(&self) -> Vec<TextureId> {
        let r = vec![self.button_idle, self.button_hover, self.button_click];
        r
    }
}

pub fn get_asset_path(string: &str) -> AssetPath<'_> {
    let ship_path = Path::new(string);
    let ship_source = AssetSourceId::from("embedded");
    let asset_path = AssetPath::from_path(&ship_path).with_source(ship_source);
    asset_path
}


pub fn my_button(
    ui: &mut Ui,
    name: &str,
    textures: &Vec<TextureId>,
    desired_size: Vec2,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click());

    if ui.is_rect_visible(rect) {
        // 1. 先根据状态选择合适的纹理
        let texture_index = if response.is_pointer_button_down_on() {
            2 // 按下状态
        } else if response.hovered() {
            1 // 悬停状态
        } else {
            0 // 默认状态
        };

        // 2. 绘制背景图片
        ui.painter().image(
            textures[texture_index],
            rect,
            bevy_egui::egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        // 3. 最后绘制文本（在图片上方）
        let text_color = if response.hovered() {
            Color32::BLACK
        } else {
            Color32::from_gray(200)
        };

        ui.painter().text(
            rect.center(),
            bevy_egui::egui::Align2::CENTER_CENTER,
            name,
            bevy_egui::egui::FontId::default(),
            text_color,
        );
    }

    response
}
