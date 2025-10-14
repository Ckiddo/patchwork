use std::collections::VecDeque;

use bevy::{ecs::query::QueryEntityError, prelude::*};

use crate::{
    game::WIDTH_BASE,
    new_game::{event::PatchChoosedEvent, generate_color, mid_pos},
};

pub struct Patch {
    shape: Vec<usize>,
    bt: (usize, usize),
    button: usize,
}

pub fn new_patches() -> Vec<Patch> {
    let patches = vec![
        Patch {
            shape: vec![1, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1],
            bt: (1, 2),
            button: 0,
        },
        Patch {
            shape: vec![0, 1, 0, 1, 1, 1, 0, 1, 0, 0, 1],
            bt: (0, 3),
            button: 1,
        },
        Patch {
            shape: vec![0, 0, 1, 0, 1, 1, 1, 1],
            bt: (10, 4),
            button: 3,
        },
        Patch {
            shape: vec![1, 0, 0, 1, 1, 0, 1, 0, 0, 1],
            bt: (3, 4),
            button: 1,
        },
        Patch {
            shape: vec![1, 1, 0, 0, 1],
            bt: (3, 1),
            button: 1,
        },
        Patch {
            shape: vec![1, 1, 1, 0, 1, 0, 1, 1, 1],
            bt: (2, 3),
            button: 0,
        },
        Patch {
            shape: vec![0, 1, 0, 1, 1, 0, 1, 1, 0, 1],
            bt: (4, 2),
            button: 0,
        },
        Patch {
            shape: vec![1, 1, 1, 1, 1],
            bt: (2, 2),
            button: 0,
        },
        Patch {
            shape: vec![0, 1, 1, 1, 1, 0, 0, 1, 1],
            bt: (3, 6),
            button: 0,
        },
        Patch {
            shape: vec![1, 1],
            bt: (2, 1),
            button: 0,
        },
        Patch {
            shape: vec![1, 1, 0, 1, 0, 0, 1, 0, 0, 1],
            bt: (10, 3),
            button: 2,
        },
        Patch {
            shape: vec![0, 1, 0, 1, 1, 1, 0, 1],
            bt: (5, 4),
            button: 2,
        },
        Patch {
            shape: vec![1, 1, 1, 0, 1, 0, 0, 1, 0, 0, 1],
            bt: (7, 2),
            button: 2,
        },
        Patch {
            shape: vec![0, 1, 0, 1, 1, 0, 0, 1, 1, 0, 1],
            bt: (2, 1),
            button: 0,
        },
        Patch {
            shape: vec![1, 1, 0, 1, 1, 1, 0, 0, 1],
            bt: (8, 6),
            button: 3,
        },
        Patch {
            shape: vec![1, 0, 0, 1, 1, 0, 1, 1, 0, 1],
            bt: (7, 4),
            button: 2,
        },
        Patch {
            shape: vec![1, 1, 0, 0, 1, 0, 0, 1],
            bt: (4, 6),
            button: 2,
        },
        Patch {
            shape: vec![0, 1, 0, 0, 1, 0, 1, 1, 1, 0, 1, 0, 0, 1],
            bt: (1, 4),
            button: 1,
        },
        Patch {
            shape: vec![1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1],
            bt: (1, 5),
            button: 1,
        },
        Patch {
            shape: vec![1, 1, 0, 0, 1],
            bt: (1, 3),
            button: 0,
        },
        Patch {
            shape: vec![1, 0, 0, 1, 0, 0, 1, 0, 0, 1],
            bt: (3, 3),
            button: 1,
        },
        Patch {
            shape: vec![1, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1],
            bt: (2, 3),
            button: 1,
        },
        Patch {
            shape: vec![0, 1, 1, 1, 1],
            bt: (3, 2),
            button: 1,
        },
        Patch {
            shape: vec![1, 1, 1, 1],
            bt: (4, 2),
            button: 1,
        },
        Patch {
            shape: vec![1, 1, 1, 1, 0, 1],
            bt: (1, 2),
            button: 0,
        },
        Patch {
            shape: vec![1, 1, 0, 0, 1, 1],
            bt: (7, 6),
            button: 3,
        },
        Patch {
            shape: vec![0, 1, 0, 1, 1, 1, 1, 1, 1, 0, 1],
            bt: (5, 3),
            button: 1,
        },
        Patch {
            shape: vec![1, 1, 0, 1, 1, 0, 1, 0, 0, 1],
            bt: (10, 5),
            button: 3,
        },
        Patch {
            shape: vec![1, 0, 0, 1, 1, 1, 1],
            bt: (5, 5),
            button: 2,
        },
        Patch {
            shape: vec![1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1],
            bt: (7, 1),
            button: 1,
        },
        Patch {
            shape: vec![1, 1, 1, 0, 1],
            bt: (2, 2),
            button: 0,
        },
        Patch {
            shape: vec![1, 1, 0, 1, 1],
            bt: (6, 5),
            button: 2,
        },
        Patch {
            shape: vec![1, 1, 1],
            bt: (2, 2),
            button: 0,
        },
    ];
    info!("patches len: {}", &patches.len());
    patches
}

pub fn generate_perimeter_positions(n: usize) -> VecDeque<bevy_egui::egui::Vec2> {
    let start_j = 7;
    let mut i = 0;
    let mut j = start_j;
    let square_size = 120.0;

    let mut idx = 0;
    let mut ret = VecDeque::new();
    while j < 15 && idx < n {
        ret.push_back(mid_pos(i, j, square_size));
        j += 1;
        idx += 1;
    }
    while i < 8 && idx < n {
        ret.push_back(mid_pos(i, j, square_size));
        i += 1;
        idx += 1;
    }
    while j >= 1 && idx < n {
        ret.push_back(mid_pos(i, j, square_size));
        j -= 1;
        idx += 1;
    }
    while i >= 1 && idx < n {
        ret.push_back(mid_pos(i, j, square_size));
        i -= 1;
        idx += 1;
    }
    while j < start_j && idx < n {
        ret.push_back(mid_pos(i, j, square_size));
        j += 1;
        idx += 1;
    }
    ret
}

// 记载哪个patch的Component
#[derive(Component)]
struct PatchComponent {
    pub patch_idx: usize,
}

fn inner_handle_query_entity_error(e: QueryEntityError) {
    warn!("click choose shape err: {:?}", e);
}

fn on_click_choose_shape(
    click: On<Pointer<Click>>,
    query: Query<&PatchComponent>,
    mut commands: Commands,
) {
    let e = click.event().entity;
    let c = query.get(e);
    match c {
        Ok(pc) => {
            info!("on click choose shape: {:?}", pc.patch_idx);
            commands.trigger(PatchChoosedEvent {
                patch_idx: pc.patch_idx,
            });
        }
        Err(e) => {
            inner_handle_query_entity_error(e);
        }
    }
}

fn spawn_patch(
    commands: &mut Commands,
    idx: usize,
    (patch, x, y): (&Patch, f32, f32),
    root_entity: Entity,
) {
    let square_size = WIDTH_BASE / 5.0;
    let color = Color::linear_rgba(0., 0., 0., 0.);

    // 透明sprite用于点选
    let p = commands
        .spawn((
            Sprite {
                color,
                custom_size: Some(Vec2::splat(WIDTH_BASE)),
                ..default()
            },
            Transform::from_xyz(x, y, 0.0),
            Pickable::default(),
            PatchComponent { patch_idx: idx },
        ))
        .observe(on_click_choose_shape)
        .id();

    commands.entity(root_entity).add_child(p);

    // 在透明Sprite上画形状
    for (pos, &has) in patch.shape.iter().enumerate() {
        let row = pos / 3;
        let col = pos % 3;
        let color = generate_color(row as i32, col as i32, 5, 3);
        let x = col as f32 * square_size + square_size / 2.0 - WIDTH_BASE / 2.0;
        let y = row as f32 * square_size + square_size / 2.0 - WIDTH_BASE / 2.0;
        if has == 1 {
            let c = commands
                .spawn((
                    Sprite {
                        color,
                        custom_size: Some(Vec2::splat(square_size)),
                        ..default()
                    },
                    Transform::from_xyz(x, y, 0.1),
                ))
                .id();
            commands.entity(p).add_child(c);
        }
    }
}

// 外面一圈的patches
pub fn spawn_patches(commands: &mut Commands,patches: &Vec<Patch>, root_entity: Entity) {
    // 先设定好各个patches的位置
    // let patches = new_patches();
    let pos = generate_perimeter_positions(patches.len());

    // 放置 各个patches
    for (idx, &bevy_egui::egui::Vec2 { x, y }) in pos.iter().enumerate() {
        spawn_patch(commands, idx, (&patches[idx], x, y), root_entity);
    }
}
