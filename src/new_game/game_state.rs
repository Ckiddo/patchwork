use bevy::{math::Vec2, prelude::*};
use bevy_egui::{
    EguiContexts, EguiTextureHandle, EguiUserTextures,
    egui::{self, Align2, Id, vec2},
};

use crate::{
    game::{HEIGHT, WIDTH, WIDTH_BASE},
    new_game::{
        chessboard::spawn_chessboard,
        generate_color, mid_pos,
        patches::{Patch, new_patches, spawn_patches},
    },
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

// In game
// 每次进入game 都初始化一个新的游戏资源
// 布置sprite 场景
// 设定好每个sprite 的 事件
pub fn init_game_resource(mut commands: Commands) {
    let r = BoardGame::new();
    commands.insert_resource(r);

    // 放置patches
    spawn_patches(&mut commands);

    // 放置棋盘
    let color1 = Color::srgb_u8(128, 128, 128);
    let color2 = Color::srgb_u8(73, 73, 73);
    spawn_chessboard(&mut commands, 7.0 * 60.0, 0.0, color1, color2);

    // 放置棋盘
    let color1 = Color::srgb_u8(116, 218, 255);
    let color2 = Color::srgb_u8(78, 208, 255);
    spawn_chessboard(&mut commands, -7.0 * 60.0, 0.0, color1, color2);
}
