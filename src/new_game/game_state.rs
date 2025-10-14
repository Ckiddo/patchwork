use bevy::{math::U8Vec2, prelude::*};
use bevy_egui::{
    EguiContexts, EguiTextureHandle, EguiUserTextures,
    egui::{self, Align2, Id, vec2},
};

use crate::{
    game::WIDTH_BASE,
    new_game::{
        chessboard::{spawn_chessboard, BlockInfo, PreSelectDrawer, PutShapeDrawer},
        patches::{new_patches, spawn_patches, Patch},
    },
    ui::{get_asset_path, my_button, HelloUiTextures},
};

// GameState

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, States)]
pub enum GameState {
    #[default]
    HelloUI,
    InGame,
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

#[derive(Clone)]
pub enum ShapeDirection {
    East,
    South,
    West,
    North,
}
#[derive(Resource)]
pub struct InteractiveInfo {
    pub choosing_shape: Option<usize>,
    pub choosing_shape_dir: ShapeDirection,
}

#[derive(Resource)]
pub struct BoardGame {
    // root component
    root_entity: Entity,

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
    pub patches: Vec<Patch>,
    // 拼布放置的位置
    patch_pos: Vec<Option<U8Vec2>>,
    // 拼布占据的格子
    patch_occ: Vec<Vec<bool>>,
    // 77 板块
}


pub struct ChessBoardProperty {
    pub root_entity: Entity,
    pub pos_x: f32,
    pub pos_y: f32,
    pub color1: Color,
    pub color2: Color,
}

impl BoardGame {
    pub fn put(&mut self, idx: usize, offset: &BlockInfo, dir: ShapeDirection) {
        // 中央银行存款要扣除给到 玩家 todo
        // 玩家存款要根据patch 更新 todo

        // move tick 更新 todo
        // 导致的特殊布的更新 todo

        // 导致的纽扣更新 todo

        // 拼布放置的位置更新
        self.patch_pos[idx] = Some(U8Vec2 {
            x: offset.col as u8,
            y: offset.row as u8,
        });
        // 占据的格子的更新
        self.patches[idx].get_pos((offset.col as isize, offset.row as isize), dir).iter().for_each(|&(x,y)|{
            if x < 0 || x >= 9 || y < 0 || y >= 9 {
                return;
            }
            self.patch_occ[x as usize][y as usize] = true;
        });
    }
    pub fn can_put(&self, idx: usize, offset: (usize, usize), dir: ShapeDirection) -> bool {
        // 校验 idx 范围
        if idx > self.patches.len() {
            warn!("can put fail: {} > {}", idx, self.patches.len());
            return false;
        }

        // 校验是否放置
        if self.patch_pos[idx].is_some() {
            warn!("can put fail: {} already put", idx);
            return false;
        }

        // 校验交叉
        let offset = (offset.0 as isize, offset.1 as isize);
        if self.patches[idx]
            .get_pos(offset, dir)
            .iter()
            .map(|&(x, y)| {
                if x < 0 || x >= 9 || y < 0 || y >= 9 {
                    return true;
                }
                self.patch_occ[x as usize][y as usize]
            })
            .any(|b| b)
        {
            warn!("can put fail: cross {}", idx);
            return false;
        }
        return true;
    }
    pub fn new(e: Entity) -> Self {
        let patches = new_patches();
        Self {
            patch_occ: vec![vec![false; 9]; 9],
            root_entity: e,
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
            special_patches: vec![49, 43, 31, 25, 19],
            button_pos: [4, 10, 16, 22, 28, 34, 40, 46, 52],
            patch_pos: vec![None; patches.len()],
            patches: patches,
        }
    }
}

// In game
// 每次进入game 都初始化一个新的游戏资源
// 布置sprite 场景
// 设定好每个sprite 的 事件
pub fn init_game_resource(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let root_entity = commands.spawn(Transform::from_xyz(0.0, 0.0, 0.0)).id();
    let r = BoardGame::new(root_entity);

    // 放置patches
    let shape = meshes.add(Triangle2d::new(
        bevy::math::vec2(0.0, 0.0),
        bevy::math::vec2(WIDTH_BASE / 5.0, WIDTH_BASE / 5.0),
        bevy::math::vec2(-WIDTH_BASE / 5.0, WIDTH_BASE / 5.0),
    ));
    let color = materials.add(Color::linear_rgb(0.0, 1.0, 0.0));
    spawn_patches(&mut commands, &r.patches, root_entity, shape, color);

    // 放置棋盘
    let cbp = ChessBoardProperty {
        root_entity,
        pos_x: 7.0 * 60.0,
        pos_y: 0.0,
        color1: Color::srgb_u8(128, 128, 128),
        color2: Color::srgb_u8(73, 73, 73),
    };
    spawn_chessboard(&mut commands, cbp);

    // 放置棋盘
    let cbp = ChessBoardProperty {
        root_entity,
        pos_x: -7.0 * 60.0,
        pos_y: 0.0,
        color1: Color::srgb_u8(116, 218, 255),
        color2: Color::srgb_u8(78, 208, 255),
    };
    spawn_chessboard(&mut commands, cbp);

    commands.insert_resource(r);

    // 前端交互资源
    commands.insert_resource(InteractiveInfo {
        choosing_shape: None,
        choosing_shape_dir: ShapeDirection::East,
    });

    // 用于提示放置位置的Component
    let t = commands.spawn((PreSelectDrawer, Transform::default())).id();
    commands.entity(root_entity).add_child(t);
    
    // 已经放置的形状
    let t = commands.spawn((PutShapeDrawer, Transform::default())).id();
    commands.entity(root_entity).add_child(t);
}

pub fn del_game_component(mut commands: Commands, res: Res<BoardGame>) {
    let e = res.root_entity;
    commands.entity(e).despawn();
    commands.remove_resource::<BoardGame>();
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
