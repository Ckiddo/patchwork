use bevy::prelude::*;
use bevy_egui::{
    EguiContexts, EguiTextureHandle, EguiUserTextures,
    egui::{self, Align2, Id, vec2},
};

use crate::{
    game::{HEIGHT, WIDTH, WIDTH_BASE},
    ui::{get_asset_path, my_button, HelloUiTextures},
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

enum BoardType {
    // Yellow,
    Blue,
}
enum TimeBoardType {
    // Circle,
    Square,
}

struct Player {
    // 玩家存款 5元
    money: usize,
    // 玩家指示物 位置
    pos_idx: usize,
    // move tick 表示在哪个tick执行了移动
    last_move_tick: usize,
}

struct Patch {
    shape: Vec<usize>,
    bt: (usize, usize),
    button: usize,
}

#[derive(Resource)]
struct BoardGame {
    // 每个玩家的拼布图版的样式
    board_type: BoardType,

    // 中央时间板的样式
    time_board_type: TimeBoardType,

    // 中央银行的存款 32 + 12 * 5 + 5 * 10 + 1 * 20
    bank_money: usize,

    // player // 53的位置结束
    players: [Player; 2],

    // 标识当前move的 tick
    global_move_tick: usize,

    // 当前行动的玩家 idx
    // player: usize,

    // 特殊布的位置 19 25 31 43 49 反着放 pop尾部
    special_patches: Vec<usize>,

    // 棋盘上纽扣的位置 4 10 16 22 28 34 40 46 52
    button_pos: [usize; 9],

    // 拼布的随机初始化
    patches: Vec<Patch>,
    // 77 板块
}

impl BoardGame {
    pub fn new() -> Self {
        Self {
            board_type: BoardType::Blue,
            time_board_type: TimeBoardType::Square,
            bank_money: 32 + 12 * 5 + 5 * 10 + 1 * 20,
            players: [
                Player {
                    money: 5,
                    pos_idx: 0,
                    last_move_tick: 1,
                },
                Player {
                    money: 5,
                    pos_idx: 0,
                    last_move_tick: 0,
                },
            ],
            global_move_tick: 2, // move_tick从2开始计数 谁移动了，谁的last move_tick就设置成global move_tick，之后global_move + 1
            // player: ,
            special_patches: vec![49, 43, 31, 25, 19],
            button_pos: [4, 10, 16, 22, 28, 34, 40, 46, 52],
            patches: new_patches(),
        }
    }
}

fn new_patches() -> Vec<Patch> {
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

// In game
// 每次进入game 都初始化一个新的游戏资源
// 布置sprite 场景
// 设定好每个sprite 的 事件
pub fn init_game_resource(mut commands: Commands) {
    let r = BoardGame::new();
    commands.insert_resource(r);

    spawn_grid(&mut commands);
}

fn spawn_grid(commands: &mut Commands) {
    let square_size = 120.0;
    let world_width = WIDTH;
    let world_height = HEIGHT;

    // 计算可以放置多少个方块
    let cols = (world_width / square_size) as i32; // 16 列
    let rows = (world_height / square_size) as i32; // 9 行

    for row in 0..rows {
        for col in 0..cols {
            // 计算方块中心位置（考虑到坐标从 (0,0) 到 (1920, 1080)）
            let x = col as f32 * square_size + square_size / 2.0 - WIDTH / 2.0;
            let y = row as f32 * square_size + square_size / 2.0 - HEIGHT / 2.0;

            // 生成随机或渐变颜色
            let color = generate_color(row, col, rows, cols);

            commands.spawn((
                Sprite {
                    color,
                    custom_size: Some(Vec2::splat(square_size)),
                    ..default()
                },
                Transform::from_xyz(x, y, 0.0),
            ));
        }
    }
}
fn generate_color(row: i32, col: i32, rows: i32, cols: i32) -> Color {
    let index = (row * cols + col) as f32;
    let total = (rows * cols) as f32;
    let hue = (index / total) * 360.0;
    Color::hsl(hue, 0.8, 0.6)
}
