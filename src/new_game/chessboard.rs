use bevy::prelude::*;

use crate::new_game::game_state::ChessBoardProperty;
pub fn spawn_chessboard(commands: &mut Commands, cbp: ChessBoardProperty) {
    // let width = 7.0 * 120.0; // 棋盘外边框的长度
    let width = 6.0 * 120.0; // 棋盘外边框的长度
    let square_size = width / 9.0; // 9个格子
    let rows = 9;
    let cols = 9;

    for row in 0..rows {
        for col in 0..cols {
            // 计算方块中心位置（考虑到坐标从 (0,0) 到 (1920, 1080)）
            let x = col as f32 * square_size + square_size / 2.0 + cbp.pos_x - width / 2.0;
            let y = row as f32 * square_size + square_size / 2.0 + cbp.pos_y - width / 2.0;

            let c = commands
                .spawn((
                    Sprite {
                        // color: Color::srgb_u8(128, 128, 128),
                        color: cbp.color1,
                        custom_size: Some(Vec2::splat(square_size)),
                        ..Default::default()
                    },
                    Transform::from_xyz(x, y, 0.0),
                ))
                .with_child((
                    Sprite {
                        // color: Color::srgb_u8(73, 73, 73),
                        color: cbp.color2,
                        custom_size: Some(Vec2::splat(square_size * 0.95)),
                        ..default()
                    },
                    Transform::from_xyz(0.0, 0.0, 0.1),
                ))
                .id();

            commands.entity(cbp.root_entity).add_child(c);
        }
    }
}
