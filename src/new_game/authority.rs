//! Persistent Bevy scene driven only by committed server snapshots and a separate draft.
use crate::game_view::Draft;
use bevy::prelude::*;
use game_core::{
    BoardPosition, Seat,
    geometry::{Orientation, patch_shape},
    rules::{PatchId, patch},
    state::{ActionPhase, GameSnapshot, NeutralPosition, PieceId},
};
use std::cell::RefCell;

#[derive(Clone, PartialEq)]
pub(crate) struct SceneInput {
    pub snapshot: GameSnapshot,
    pub own: Seat,
    pub version: u64,
    pub draft: Draft,
    pub active: bool,
    pub valid: bool,
}
#[derive(Clone, Copy)]
pub(crate) enum SceneEvent {
    Select(PatchId),
    Anchor(BoardPosition),
    PlaceAt(BoardPosition),
}
#[derive(Default)]
struct Bridge {
    revision: u64,
    input: Option<SceneInput>,
    handler: yew::Callback<SceneEvent>,
}
thread_local! {
    static BRIDGE:RefCell<Bridge>=RefCell::new(Bridge::default());
    static REDUCED_MOTION: Option<web_sys::MediaQueryList> = web_sys::window()
        .and_then(|w| w.match_media("(prefers-reduced-motion: reduce)").ok().flatten());
}
pub(crate) fn publish(input: Option<SceneInput>, handler: yew::Callback<SceneEvent>) {
    BRIDGE.with_borrow_mut(|b| {
        if b.input != input {
            b.input = input;
            b.revision = b.revision.wrapping_add(1);
        }
        b.handler = handler;
    });
}
fn emit(event: SceneEvent) {
    let callback = BRIDGE.with_borrow(|b| b.handler.clone());
    callback.emit(event);
}
#[derive(Component)]
pub struct QuiltBoard {
    pub owner: Seat,
}
#[derive(Component)]
pub struct QuiltCell {
    pub owner: Seat,
    pub x: u8,
    pub y: u8,
}
#[derive(Component)]
pub struct PlacedPatchView {
    pub owner: Seat,
    pub piece: PieceId,
}
#[derive(Component)]
struct CellFill {
    owner: Seat,
    x: u8,
    y: u8,
}
#[derive(Component)]
pub struct PatchIncomeLabel {
    pub owner: Seat,
    pub patch_id: PatchId,
}
#[derive(Component)]
pub struct PatchView {
    pub patch_id: PatchId,
    pub slot: usize,
}
#[derive(Component)]
pub struct CandidateFrame {
    pub patch_id: PatchId,
}
#[derive(Component)]
struct SelectedFrame {
    patch_id: PatchId,
}
#[derive(Component)]
pub struct NeutralMarker;
#[derive(Component)]
pub struct PlacementPreview {
    index: usize,
}
#[derive(Resource, Default)]
struct Rendered {
    revision: u64,
    game: Option<String>,
    version: Option<u64>,
    root: Option<Entity>,
}
pub struct AuthorityPlugin;
impl Plugin for AuthorityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Rendered>()
            .insert_resource(ClearColor(Color::srgb_u8(24, 59, 66)))
            .add_systems(Update, (synchronize_scene, pulse_candidates).chain());
    }
}
pub(super) fn board_click(e: On<Pointer<Click>>, cells: Query<&QuiltCell>) {
    if e.button != bevy::picking::pointer::PointerButton::Primary {
        return;
    }
    if let Ok(cell) = cells.get(e.entity) {
        point_at(cell, SceneEvent::PlaceAt);
    }
}
pub(super) fn board_hover(e: On<Pointer<Over>>, cells: Query<&QuiltCell>) {
    if let Ok(cell) = cells.get(e.entity) {
        point_at(cell, SceneEvent::Anchor);
    }
}
fn point_at(cell: &QuiltCell, event: fn(BoardPosition) -> SceneEvent) {
    let allowed=BRIDGE.with_borrow(|b|b.input.as_ref().is_some_and(|s|s.active&&s.own==cell.owner&&(s.draft.patch.is_some()||matches!(s.snapshot.action_phase(),ActionPhase::Special{actor,..}if actor==s.own))));
    if allowed {
        emit(event(
            BoardPosition::new(cell.x.into(), cell.y.into()).unwrap(),
        ));
    }
}
fn patch_click(e: On<Pointer<Click>>, patches: Query<&PatchView>) {
    // A secondary click cancels through the game area's contextmenu handler.
    // Its later Pointer<Click> must not select a patch again.
    if e.button != bevy::picking::pointer::PointerButton::Primary {
        return;
    }
    let Ok(p) = patches.get(e.entity) else { return };
    let allowed = BRIDGE.with_borrow(|b| {
        b.input.as_ref().is_some_and(|s| {
            s.active
                && matches!(s.snapshot.action_phase(), ActionPhase::Normal { .. })
                && s.snapshot.supply().candidates().contains(&p.patch_id)
        })
    });
    if allowed {
        emit(SceneEvent::Select(p.patch_id));
    }
}
pub(super) fn patch_color(id: PatchId) -> Color {
    Color::hsl(((id.0 * 137) % 360) as f32, 0.56, 0.58)
}
// Choose an occupied cell near the piece's center so concave shapes never label a hole.
fn income_label_cell(cells: &[BoardPosition]) -> Option<BoardPosition> {
    let count = cells.len() as i32;
    let (sx, sy) = cells.iter().fold((0, 0), |(sx, sy), cell| {
        let (x, y) = cell.coordinates();
        (sx + i32::from(x), sy + i32::from(y))
    });
    cells.iter().copied().min_by_key(|cell| {
        let (x, y) = cell.coordinates();
        let dx = i32::from(x) * count - sx;
        let dy = i32::from(y) * count - sy;
        (dx * dx + dy * dy, y, x)
    })
}
fn spawn_frame(
    commands: &mut Commands,
    parent: Entity,
    size: f32,
    color: Color,
    id: PatchId,
    candidate: bool,
) {
    for (x, y, w, h) in [
        (0., size / 2., size, 4.),
        (0., -size / 2., size, 4.),
        (-size / 2., 0., 4., size),
        (size / 2., 0., 4., size),
    ] {
        let mut entity = commands.spawn((
            Sprite::from_color(color, Vec2::new(w, h)),
            Transform::from_xyz(x, y, 0.6),
            Visibility::Hidden,
            Pickable::IGNORE,
        ));
        if candidate {
            entity.insert(CandidateFrame { patch_id: id });
        } else {
            entity.insert(SelectedFrame { patch_id: id });
        }
        let e = entity.id();
        commands.entity(parent).add_child(e);
    }
}
fn spawn_scene(commands: &mut Commands, s: &SceneInput) -> Entity {
    let root = commands
        .spawn((Transform::default(), Visibility::Visible))
        .id();
    for (mine, owner) in [(false, s.own.other()), (true, s.own)] {
        let board = super::chessboard::spawn_authoritative_board(commands, root, owner, mine);
        commands.entity(board).insert(QuiltBoard { owner });
        for y in 0..9u8 {
            for x in 0..9u8 {
                let pos = super::chessboard::quilt_cell_position(x, y);
                let fill = commands
                    .spawn((
                        Sprite::from_color(Color::WHITE, Vec2::splat(76.)),
                        Transform::from_xyz(pos.x, pos.y, 0.3),
                        Visibility::Hidden,
                        CellFill { owner, x, y },
                        Pickable::IGNORE,
                    ))
                    .id();
                commands.entity(board).add_child(fill);
            }
        }
        // One reusable badge per income-producing patch, not one per occupied cell.
        for &id in s.snapshot.supply().initial_order() {
            let definition = patch(id).unwrap();
            if definition.income == 0 {
                continue;
            }
            let label = commands
                .spawn((
                    Sprite::from_color(Color::srgb_u8(30, 49, 53), Vec2::new(66., 52.)),
                    Transform::default(),
                    Visibility::Hidden,
                    PatchIncomeLabel {
                        owner,
                        patch_id: id,
                    },
                    Pickable::IGNORE,
                ))
                .with_child((
                    Text2d::new(format!("+{}", definition.income)),
                    TextFont {
                        font_size: 42.,
                        ..default()
                    },
                    TextColor(Color::srgb_u8(255, 229, 155)),
                    Transform::from_xyz(0., 0., 0.1),
                    Pickable::IGNORE,
                ))
                .id();
            commands.entity(board).add_child(label);
        }
    }
    let positions = super::patches::generate_perimeter_positions(33);
    for (slot, &id) in s.snapshot.supply().initial_order().iter().enumerate() {
        let pos = positions[slot];
        let parent = commands
            .spawn((
                Sprite::from_color(Color::srgb_u8(26, 47, 56), Vec2::splat(108.)),
                Transform::from_xyz(pos.x, pos.y, 0.2),
                PatchView { patch_id: id, slot },
                Pickable::default(),
            ))
            .observe(patch_click)
            .id();
        commands.entity(root).add_child(parent);
        let shape = patch_shape(id, Orientation::default()).unwrap();
        super::patches::spawn_frozen_patch_cells(
            commands,
            parent,
            shape.cells(),
            18.,
            patch_color(id),
        );
        let def = patch(id).unwrap();
        let text = commands
            .spawn((
                Text2d::new(format!(
                    "{} / {} / +{}",
                    def.button_cost, def.time_cost, def.income
                )),
                TextFont {
                    font_size: 17.,
                    ..default()
                },
                TextColor(Color::srgb_u8(230, 234, 215)),
                Transform::from_xyz(0., -44., 0.5),
                Pickable::IGNORE,
            ))
            .id();
        commands.entity(parent).add_child(text);
        spawn_frame(
            commands,
            parent,
            114.,
            Color::srgb_u8(118, 245, 146),
            id,
            true,
        );
        spawn_frame(
            commands,
            parent,
            104.,
            Color::srgb_u8(255, 225, 145),
            id,
            false,
        );
    }
    let marker = commands
        .spawn((
            Sprite::from_color(Color::srgb_u8(247, 218, 146), Vec2::splat(28.)),
            Transform::from_xyz(0., 0., 2.)
                .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4)),
            NeutralMarker,
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(root).add_child(marker);
    for index in 0..12 {
        let preview = commands
            .spawn((
                Sprite::from_color(Color::WHITE, Vec2::splat(73.)),
                Transform::from_xyz(0., 0., 1.),
                Visibility::Hidden,
                PlacementPreview { index },
                Pickable::IGNORE,
            ))
            .id();
        commands.entity(root).add_child(preview);
    }
    root
}
#[allow(clippy::too_many_arguments)]
fn synchronize_scene(
    mut commands: Commands,
    mut rendered: ResMut<Rendered>,
    patches: Query<(Entity, &PatchView)>,
    frames: Query<(Entity, &CandidateFrame)>,
    selected: Query<(Entity, &SelectedFrame)>,
    fills: Query<(Entity, &CellFill)>,
    income_labels: Query<(Entity, &PatchIncomeLabel)>,
    neutral: Query<Entity, With<NeutralMarker>>,
    previews: Query<(Entity, &PlacementPreview)>,
) {
    let change = BRIDGE
        .with_borrow(|b| (b.revision != rendered.revision).then(|| (b.revision, b.input.clone())));
    let Some((revision, input)) = change else {
        return;
    };
    let Some(s) = input else {
        if let Some(root) = rendered.root.take() {
            commands.entity(root).despawn();
        }
        rendered.game = None;
        rendered.version = None;
        rendered.revision = revision;
        return;
    };
    if rendered.game.as_deref() != Some(s.snapshot.game_id()) {
        if let Some(root) = rendered.root.take() {
            commands.entity(root).despawn();
        }
        rendered.root = Some(spawn_scene(&mut commands, &s));
        rendered.game = Some(s.snapshot.game_id().into());
        rendered.version = None;
        return; // Finish state after deferred spawn commands become queryable next frame.
    }
    if rendered.version != Some(s.version) {
        for (entity, p) in &patches {
            commands
                .entity(entity)
                .insert(if s.snapshot.supply().slots()[p.slot].is_some() {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                });
        }
        for (entity, cell) in &fills {
            let piece = s.snapshot.player(cell.owner).board().at(BoardPosition::new(
                cell.x.into(),
                cell.y.into(),
            )
            .unwrap());
            if let Some(piece) = piece {
                let color = match piece {
                    PieceId::Normal(id) => patch_color(id),
                    PieceId::Special(_) => Color::srgb_u8(219, 190, 141),
                };
                commands.entity(entity).insert((
                    Sprite::from_color(color, Vec2::splat(76.)),
                    Visibility::Visible,
                    PlacedPatchView {
                        owner: cell.owner,
                        piece,
                    },
                ));
            } else {
                commands
                    .entity(entity)
                    .insert(Visibility::Hidden)
                    .remove::<PlacedPatchView>();
            }
        }
        let candidates = s.snapshot.supply().candidates();
        for (entity, label) in &income_labels {
            let cell = s
                .snapshot
                .player(label.owner)
                .placed_pieces()
                .iter()
                .find(|piece| piece.piece() == PieceId::Normal(label.patch_id))
                .and_then(|piece| income_label_cell(piece.cells()));
            if let Some(cell) = cell {
                let (x, y) = cell.coordinates();
                let position = super::chessboard::quilt_cell_position(x, y);
                commands.entity(entity).insert((
                    Transform::from_xyz(position.x, position.y, 1.2),
                    Visibility::Visible,
                ));
            } else {
                commands.entity(entity).insert(Visibility::Hidden);
            }
        }
        for (entity, frame) in &frames {
            commands
                .entity(entity)
                .insert(if candidates.contains(&frame.patch_id) {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                });
        }
        let positions = super::patches::generate_perimeter_positions(33);
        let pos = match s.snapshot.supply().neutral() {
            NeutralPosition::BeforeSlot { slot } => {
                let a = positions[usize::from(slot)];
                let b = positions[(usize::from(slot) + 32) % 33];
                (a + b) * 0.5
            }
            NeutralPosition::OnVacatedSlot { slot } => positions[usize::from(slot)],
        };
        for entity in &neutral {
            commands.entity(entity).insert(
                Transform::from_xyz(pos.x, pos.y, 2.)
                    .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4)),
            );
        }
        rendered.version = Some(s.version);
    }
    for (entity, frame) in &selected {
        commands
            .entity(entity)
            .insert(if s.draft.patch == Some(frame.patch_id) {
                Visibility::Visible
            } else {
                Visibility::Hidden
            });
    }
    let special = matches!(s.snapshot.action_phase(),ActionPhase::Special{actor,..}if actor==s.own);
    let offsets = if special {
        vec![(0, 0)]
    } else {
        s.draft
            .patch
            .and_then(|id| patch_shape(id, s.draft.orientation).ok())
            .map(|shape| shape.cells().to_vec())
            .unwrap_or_default()
    };
    for (entity, preview) in &previews {
        let cell = s.draft.anchor.and_then(|a| {
            offsets.get(preview.index).map(|&(x, y)| {
                let (ax, ay) = a.coordinates();
                (ax + x, ay + y)
            })
        });
        if let Some((x, y)) = cell.filter(|(x, y)| *x < 9 && *y < 9) {
            let p = super::chessboard::quilt_cell_position(x, y);
            commands.entity(entity).insert((
                Transform::from_xyz(p.x + 420., p.y, 1.),
                Visibility::Visible,
                Sprite::from_color(
                    if s.valid && s.active {
                        Color::srgba(0.5, 1., 0.65, 0.65)
                    } else {
                        Color::srgba(1., 0.3, 0.25, 0.75)
                    },
                    Vec2::splat(73.),
                ),
            ));
        } else {
            commands.entity(entity).insert(Visibility::Hidden);
        }
    }
    rendered.revision = revision;
}
fn pulse_candidates(time: Res<Time>, mut frames: Query<&mut Sprite, With<CandidateFrame>>) {
    let reduced = REDUCED_MOTION.with(|m| m.as_ref().is_some_and(|m| m.matches()));
    let alpha = if reduced {
        1.0
    } else {
        0.65 + 0.3 * (time.elapsed_secs() * std::f32::consts::TAU / 1.6).sin()
    };
    for mut sprite in &mut frames {
        sprite.color.set_alpha(alpha);
    }
}
