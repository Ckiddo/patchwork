use bevy::prelude::*;

use crate::new_game::{
    game_state::{BoardGame, ChessBoardProperty, InteractiveInfo},
    patches::{ShapeChooseMark, inner_handle_query_entity_error},
};

// 画那些要放在棋盘上的形状
#[derive(Component)]
pub struct PreSelectDrawer;

// 画那些已经放上去的形状
#[derive(Component)]
pub struct PutShapeDrawer;

// 给棋盘的格子标记位置
#[derive(Component)]
pub struct BlockInfo {
    pub col: usize,
    pub row: usize,
}

/// Same 720-unit board as the original scene; core coordinates go right and down.
pub fn quilt_cell_position(x: u8, y: u8) -> Vec2 {
    Vec2::new(
        f32::from(x) * 80. + 40. - 360.,
        360. - f32::from(y) * 80. - 40.,
    )
}
pub fn spawn_authoritative_board(
    commands: &mut Commands,
    root: Entity,
    owner: game_core::Seat,
    mine: bool,
) -> Entity {
    use super::authority::{QuiltCell, board_click, board_hover};
    let board = commands
        .spawn((
            Transform::from_xyz(if mine { 420. } else { -420. }, 0., 0.),
            Visibility::Visible,
        ))
        .id();
    commands.entity(root).add_child(board);
    let border = commands
        .spawn((
            Sprite::from_color(
                if mine {
                    Color::srgb_u8(111, 165, 181)
                } else {
                    Color::srgb_u8(122, 151, 158)
                },
                Vec2::splat(734.),
            ),
            Transform::from_xyz(0., 0., -0.1),
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(board).add_child(border);
    for y in 0..9u8 {
        for x in 0..9u8 {
            let pos = quilt_cell_position(x, y);
            // Existing tile sprite + inset drawing; replace local write observers with intents.
            let cell = commands
                .spawn((
                    Sprite::from_color(Color::srgb_u8(78, 109, 116), Vec2::splat(80.)),
                    Transform::from_xyz(pos.x, pos.y, 0.),
                    QuiltCell { owner, x, y },
                    Pickable::default(),
                ))
                .observe(board_click)
                .observe(board_hover)
                .with_child((
                    Sprite::from_color(Color::srgb_u8(25, 51, 59), Vec2::splat(77.)),
                    Transform::from_xyz(0., 0., 0.1),
                    Pickable::IGNORE,
                ))
                .id();
            commands.entity(board).add_child(cell);
        }
    }
    board
}

fn board_on_click(
    on: On<Pointer<Click>>,
    query: Query<&BlockInfo>,
    mut int_r: ResMut<InteractiveInfo>,
    mut board: ResMut<BoardGame>,
    psd: Single<Entity, With<PreSelectDrawer>>,
    put_shape_drawer: Single<Entity, With<PutShapeDrawer>>,
    mut commands: Commands,
    mut scm: Query<&mut Visibility, With<ShapeChooseMark>>,
) {
    match query.get(on.event().entity) {
        Err(err) => {
            inner_handle_query_entity_error(err);
        }
        Ok(bi) => {
            // 执行放置

            // 校验 选没选
            if int_r.choosing_shape.is_none() {
                // 没选中: 结束
                warn!("not chose shape");
                return;
            }

            // 如果选中
            let idx = int_r.choosing_shape.unwrap();

            // 校验能放
            if !board.can_put(idx, (bi.col, bi.row), int_r.choosing_shape_dir.clone()) {
                warn!("cant put ");
                return;
            }

            // 放置
            board.put(idx, &bi, int_r.choosing_shape_dir.clone());

            // 前端记录要清除
            // 1 psd
            commands.entity(psd.entity()).despawn_children();
            // 2 int_resource
            int_r.choosing_shape = None;
            // 3 marker
            for mut v in scm.iter_mut() {
                *v = Visibility::Hidden;
            }

            // 前端放置
            let color = Color::srgb(1.0, 0.0, 0.0);
            draw_shape(
                board.as_ref(),
                idx,
                bi,
                int_r.as_mut(),
                &mut commands,
                put_shape_drawer.into_inner(),
                color,
            );
        }
    }
}

fn draw_shape(
    board: &BoardGame,
    idx: usize,
    bi: &BlockInfo,
    int_r: &InteractiveInfo,
    commands: &mut Commands,
    psd: Entity,
    color: Color,
) {
    let width = 6.0 * 120.0; // 棋盘外边框的长度
    let square_size = width / 9.0; // 9个格子
    let cbp_pos_x = 7.0 * 60.0;
    let cbp_pos_y = 0.0;
    for pos in board.patches[idx].get_pos(
        (bi.col as isize, bi.row as isize),
        int_r.choosing_shape_dir.clone(),
    ) {
        let x = pos.0 as f32 * square_size + square_size / 2.0 + cbp_pos_x - width / 2.0;
        let y = pos.1 as f32 * square_size + square_size / 2.0 + cbp_pos_y - width / 2.0;
        let t = commands
            .spawn((
                Sprite {
                    // color: Color::srgb(0.0, 0.0, 1.0),
                    color,
                    custom_size: Some(Vec2::splat(square_size * 0.9)),
                    ..default()
                },
                Transform::from_xyz(x, y, 0.2),
            ))
            .id();
        commands.entity(psd.entity()).add_child(t);
    }
}

fn board_on_hover(
    on: On<Pointer<Over>>,
    query: Query<&BlockInfo>,
    int_r: Res<InteractiveInfo>,
    board: Res<BoardGame>,
    psd: Single<Entity, With<PreSelectDrawer>>,
    mut commands: Commands,
) {
    match query.get(on.event().entity) {
        Err(err) => {
            inner_handle_query_entity_error(err);
        }
        Ok(bi) => {
            // 执行渲染：

            // 先清掉原先的
            commands.entity(psd.entity()).despawn_children();

            if int_r.choosing_shape.is_none() {
                // 没选中: 结束
                warn!("not chose shape");
                return;
            }
            // 如果选中
            let idx = int_r.choosing_shape.unwrap();

            // 校验能放
            if !board.can_put(idx, (bi.col, bi.row), int_r.choosing_shape_dir.clone()) {
                warn!("cant put ");
                return;
            }

            // 渲染
            let color = Color::srgb(0.0, 0.0, 1.0);
            draw_shape(
                board.as_ref(),
                idx,            // shape
                bi,             // offset
                int_r.as_ref(), // direction
                &mut commands,
                psd.into_inner(), // drawer father
                color,
            );
        }
    }
}

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
                        color: cbp.color1,
                        custom_size: Some(Vec2::splat(square_size)),
                        ..Default::default()
                    },
                    Transform::from_xyz(x, y, 0.0),
                    BlockInfo { row, col },
                    Pickable::default(),
                ))
                .observe(board_on_hover)
                .observe(board_on_click)
                .with_child((
                    Sprite {
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
