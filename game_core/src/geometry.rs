//! Shared geometry for native authority and WASM preview. Coordinates are x-right/y-down.
//! Reflect the base shape horizontally, rotate clockwise, then normalize the bounding box.
use crate::{
    BoardPosition, Seat,
    rules::{CUSTOM_V1, PatchId, patch},
    state::{ActionPhase, GameSnapshot, Lifecycle, PieceId, PlacedPiece, QuiltBoard},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementError {
    UnknownPiece,
    InvalidOrientation,
    OutOfBounds,
    Overlap,
    AlreadyPlaced,
    UnknownPlayer,
    WrongBoard,
    NotRunning,
    NotYourTurn,
    WrongPhase,
    NotAvailable,
    NotCandidate,
}
impl std::fmt::Display for PlacementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid placement: {self:?}")
    }
}
impl std::error::Error for PlacementError {}

/// Only 0..=3 is accepted from callers and JSON; external turns are never silently reduced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "OrientationData")]
pub struct Orientation {
    quarter_turns: u8,
    flipped: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrientationData {
    quarter_turns: u8,
    flipped: bool,
}
impl TryFrom<OrientationData> for Orientation {
    type Error = PlacementError;
    fn try_from(value: OrientationData) -> Result<Self, Self::Error> {
        Self::new(value.quarter_turns, value.flipped)
    }
}
impl Orientation {
    pub fn new(quarter_turns: u8, flipped: bool) -> Result<Self, PlacementError> {
        if quarter_turns > 3 {
            return Err(PlacementError::InvalidOrientation);
        }
        Ok(Self {
            quarter_turns,
            flipped,
        })
    }
    pub fn quarter_turns(self) -> u8 {
        self.quarter_turns
    }
    pub fn flipped(self) -> bool {
        self.flipped
    }
    pub fn rotate_clockwise(self) -> Self {
        Self {
            quarter_turns: (self.quarter_turns + 1) % 4,
            ..self
        }
    }
    pub fn rotate_counterclockwise(self) -> Self {
        Self {
            quarter_turns: (self.quarter_turns + 3) % 4,
            ..self
        }
    }
    /// Toggle the base reflection, not a reflection around the current screen orientation.
    pub fn flip(self) -> Self {
        Self {
            flipped: !self.flipped,
            ..self
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchShape {
    // Sorted by (y, x), normalized to min x = min y = 0.
    cells: Vec<(u8, u8)>,
    width: u8,
    height: u8,
}
impl PatchShape {
    pub fn cells(&self) -> &[(u8, u8)] {
        &self.cells
    }
    pub fn dimensions(&self) -> (u8, u8) {
        (self.width, self.height)
    }
    /// Anchor stays at the transformed bounding box's top-left, even if that cell is a hole.
    pub fn cells_at(&self, anchor: BoardPosition) -> Result<Vec<BoardPosition>, PlacementError> {
        let (ax, ay) = anchor.coordinates();
        self.cells
            .iter()
            .map(|&(x, y)| {
                BoardPosition::new(i32::from(ax) + i32::from(x), i32::from(ay) + i32::from(y))
                    .map_err(|_| PlacementError::OutOfBounds)
            })
            .collect()
    }
}

pub fn patch_shape(id: PatchId, orientation: Orientation) -> Result<PatchShape, PlacementError> {
    piece_shape(PieceId::Normal(id), orientation)
}

/// At most eight distinct geometries; symmetric duplicates retain the first orientation.
pub fn distinct_orientations(
    id: PatchId,
) -> Result<Vec<(Orientation, PatchShape)>, PlacementError> {
    let mut result: Vec<(Orientation, PatchShape)> = Vec::new();
    for flipped in [false, true] {
        for quarter_turns in 0..4 {
            let orientation = Orientation::new(quarter_turns, flipped)?;
            let shape = patch_shape(id, orientation)?;
            if !result.iter().any(|(_, previous)| previous == &shape) {
                result.push((orientation, shape));
            }
        }
    }
    Ok(result)
}

fn piece_shape(piece: PieceId, orientation: Orientation) -> Result<PatchShape, PlacementError> {
    let base = match piece {
        PieceId::Normal(id) => patch(id).ok_or(PlacementError::UnknownPiece)?.cells,
        PieceId::Special(position) => {
            if !CUSTOM_V1.track.special_positions.contains(&position) {
                return Err(PlacementError::UnknownPiece);
            }
            // Special patches have one canonical orientation in persisted state.
            if orientation != Orientation::default() {
                return Err(PlacementError::InvalidOrientation);
            }
            &[(0, 0)]
        }
    };
    let mut cells: Vec<(i16, i16)> = base
        .iter()
        .map(|&(x, y)| {
            let (mut x, mut y) = (i16::from(x), i16::from(y));
            if orientation.flipped {
                x = -x;
            }
            for _ in 0..orientation.quarter_turns {
                (x, y) = (-y, x);
            }
            (x, y)
        })
        .collect();
    // Only nonempty, frozen catalog shapes or the 1x1 special reach here.
    let min_x = cells.iter().map(|c| c.0).min().unwrap();
    let min_y = cells.iter().map(|c| c.1).min().unwrap();
    for (x, y) in &mut cells {
        *x -= min_x;
        *y -= min_y;
    }
    cells.sort_unstable_by_key(|&(x, y)| (y, x));
    let width = cells.iter().map(|c| c.0).max().unwrap() as u8 + 1;
    let height = cells.iter().map(|c| c.1).max().unwrap() as u8 + 1;
    Ok(PatchShape {
        cells: cells.into_iter().map(|(x, y)| (x as u8, y as u8)).collect(),
        width,
        height,
    })
}

/// Checked domain input. Wire protocol adapters supply the authenticated user separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementRequest {
    pub target_seat: Seat,
    pub piece: PieceId,
    pub anchor: BoardPosition,
    pub orientation: Orientation,
}

/// Read-only speculative result, never a token permitting a later unchecked commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementPreview {
    pub(crate) board: QuiltBoard,
    pub(crate) placed_piece: PlacedPiece,
    completed_square: Option<BoardPosition>,
}
impl PlacementPreview {
    pub fn board(&self) -> &QuiltBoard {
        &self.board
    }
    pub fn placed_piece(&self) -> &PlacedPiece {
        &self.placed_piece
    }
    /// Geometric evidence only. Award ownership and the one-time bonus belong to the action engine.
    pub fn completed_square(&self) -> Option<BoardPosition> {
        self.completed_square
    }
}

impl QuiltBoard {
    /// Geometry only. Use GameSnapshot::preview_placement for actor, phase and supply checks.
    pub fn preview_placement(
        &self,
        piece: PieceId,
        anchor: BoardPosition,
        orientation: Orientation,
    ) -> Result<PlacementPreview, PlacementError> {
        let cells = piece_shape(piece, orientation)?.cells_at(anchor)?;
        if self
            .cells
            .iter()
            .flatten()
            .any(|&occupied| occupied == Some(piece))
        {
            return Err(PlacementError::AlreadyPlaced);
        }
        if cells.iter().any(|&cell| self.at(cell).is_some()) {
            return Err(PlacementError::Overlap);
        }
        let mut board = self.clone();
        for &cell in &cells {
            let (x, y) = cell.coordinates();
            board.cells[usize::from(y)][usize::from(x)] = Some(piece);
        }
        let completed_square = board.completed_bonus_square();
        Ok(PlacementPreview {
            board,
            placed_piece: PlacedPiece {
                piece,
                cells,
                quarter_turns: orientation.quarter_turns,
                flipped: orientation.flipped,
            },
            completed_square,
        })
    }

    /// Find a completely occupied contiguous 7x7 at any of the nine possible anchors.
    pub fn completed_bonus_square(&self) -> Option<BoardPosition> {
        let side = usize::from(CUSTOM_V1.scoring.bonus_square_side);
        let size = self.cells.len();
        for y in 0..=size - side {
            for x in 0..=size - side {
                if self.cells[y..y + side]
                    .iter()
                    .all(|row| row[x..x + side].iter().all(Option::is_some))
                {
                    return Some(BoardPosition::new(x as i32, y as i32).unwrap());
                }
            }
        }
        None
    }
}

impl GameSnapshot {
    /// Pure preview against this snapshot. GameSnapshot::apply_action revalidates and handles
    /// currency, time, special consumption and the global bonus. T38 checks the stored version.
    pub fn preview_placement(
        &self,
        authenticated_user_id: &str,
        request: PlacementRequest,
    ) -> Result<PlacementPreview, PlacementError> {
        let (player, _) = self
            .perspective(authenticated_user_id)
            .ok_or(PlacementError::UnknownPlayer)?;
        if request.target_seat != player.seat() {
            return Err(PlacementError::WrongBoard);
        }
        if self.lifecycle() != Lifecycle::Running {
            return Err(PlacementError::NotRunning);
        }
        if self.input_actor() != Some(player.seat()) {
            return Err(PlacementError::NotYourTurn);
        }
        // Check catalog membership even for pieces outside the current candidate set.
        piece_shape(request.piece, request.orientation)?;
        match (self.action_phase(), request.piece) {
            (ActionPhase::Normal { .. }, PieceId::Normal(id)) => {
                if !self.supply().slots().contains(&Some(id)) {
                    return Err(PlacementError::NotAvailable);
                }
                if !self.supply().candidates().contains(&id) {
                    return Err(PlacementError::NotCandidate);
                }
            }
            (ActionPhase::Special { track_position, .. }, PieceId::Special(id))
                if track_position == id => {}
            _ => return Err(PlacementError::WrongPhase),
        }
        player
            .board()
            .preview_placement(request.piece, request.anchor, request.orientation)
    }
}

impl PlacedPiece {
    /// Compare the entire transformed shape, not just area. Record order is immaterial.
    pub(crate) fn geometry_is_valid(&self) -> bool {
        let Ok(orientation) = Orientation::new(self.quarter_turns, self.flipped) else {
            return false;
        };
        let Ok(shape) = piece_shape(self.piece, orientation) else {
            return false;
        };
        let Some(min_x) = self.cells.iter().map(|c| c.coordinates().0).min() else {
            return false;
        };
        let min_y = self.cells.iter().map(|c| c.coordinates().1).min().unwrap();
        let anchor = BoardPosition::new(i32::from(min_x), i32::from(min_y)).unwrap();
        let Ok(expected) = shape.cells_at(anchor) else {
            return false;
        };
        let mut actual: Vec<_> = self.cells.iter().map(|c| c.coordinates()).collect();
        actual.sort_unstable_by_key(|&(x, y)| (y, x));
        actual == expected.iter().map(|c| c.coordinates()).collect::<Vec<_>>()
    }
}
