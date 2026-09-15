//! Pure domain boundary. No rendering, networking, clock or database dependencies.
//! Frozen rules, validated state, shared geometry and deterministic game actions.

pub mod actions;
pub mod geometry;
pub mod rules;
pub mod state;

use serde::{Deserialize, Serialize};

pub const PLAYER_COUNT: usize = 2;
pub const BOARD_SIZE: u8 = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub enum Seat {
    First,
    Second,
}

impl From<Seat> for u32 {
    fn from(value: Seat) -> Self {
        value.index() as u32
    }
}

impl Seat {
    pub const fn other(self) -> Self {
        match self {
            Self::First => Self::Second,
            Self::Second => Self::First,
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::First => 0,
            Self::Second => 1,
        }
    }
}

impl TryFrom<u32> for Seat {
    type Error = InvalidPosition;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::First),
            1 => Ok(Self::Second),
            _ => Err(InvalidPosition),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidPosition;

impl std::fmt::Display for InvalidPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("seat or board position outside its valid range")
    }
}

/// Construction is checked; wire data must not bypass the board boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "WireBoardPosition")]
pub struct BoardPosition {
    x: u8,
    y: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireBoardPosition {
    x: i32,
    y: i32,
}

impl TryFrom<WireBoardPosition> for BoardPosition {
    type Error = &'static str;
    fn try_from(value: WireBoardPosition) -> Result<Self, Self::Error> {
        Self::new(value.x, value.y).map_err(|_| "invalid board position")
    }
}

impl BoardPosition {
    pub fn new(x: i32, y: i32) -> Result<Self, InvalidPosition> {
        if !(0..i32::from(BOARD_SIZE)).contains(&x) || !(0..i32::from(BOARD_SIZE)).contains(&y) {
            return Err(InvalidPosition);
        }
        Ok(Self {
            x: x as u8,
            y: y as u8,
        })
    }

    pub const fn coordinates(self) -> (u8, u8) {
        (self.x, self.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_external_coordinates_and_seats_outside_domain() {
        for (x, y) in [(-1, 0), (9, 0), (0, 9), (i32::MAX, i32::MIN)] {
            assert!(BoardPosition::new(x, y).is_err());
        }
        assert_eq!(BoardPosition::new(8, 8).unwrap().coordinates(), (8, 8));
        assert!(Seat::try_from(2).is_err());
        assert!(Seat::try_from(u32::MAX).is_err());
        assert_eq!(Seat::try_from(1).unwrap().index(), 1);
    }
}
