//! Read-only acceptance oracle. Validates JSON fixtures and predicts legal UI actions.
use game_core::{
    BoardPosition, Seat,
    actions::GameAction,
    geometry::Orientation,
    state::{ActionPhase, GameSnapshot},
};
use serde_json::{Value, json};
use std::io::{self, Read};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let value: Value = serde_json::from_str(&input)?;
    let state: GameSnapshot = serde_json::from_value(value["snapshot"].clone())?;
    state.validate()?;
    let checksum = serde_json::to_vec(&state)?
        .into_iter()
        .fold(0xcbf29ce484222325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
        });
    let mut legal = Vec::new();
    if let Some(actor) = state.input_actor() {
        let user = state.player(actor).user_id();
        if matches!(state.action_phase(), ActionPhase::Special { .. }) {
            'special: for y in 0..9 {
                for x in 0..9 {
                    let action = GameAction::PlaceSpecialPatch {
                        position: BoardPosition::new(x, y).unwrap(),
                    };
                    if state.apply_action(user, action).is_ok() {
                        legal.push(json!({"kind":"special","x":x,"y":y}));
                        break 'special;
                    }
                }
            }
        } else {
            for id in state.supply().candidates() {
                'piece: for flipped in [false, true] {
                    for turns in 0..4 {
                        for y in 0..9 {
                            for x in 0..9 {
                                let action = GameAction::BuyAndPlace {
                                    patch_id: id,
                                    position: BoardPosition::new(x, y).unwrap(),
                                    orientation: Orientation::new(turns, flipped)?,
                                };
                                if state.apply_action(user, action).is_ok() {
                                    legal.push(json!({"kind":"buy","id":id.0,"x":x,"y":y,"turns":turns,"flipped":flipped}));
                                    break 'piece;
                                }
                            }
                        }
                    }
                }
            }
            if state.apply_action(user, GameAction::Advance).is_ok() {
                legal.push(json!({"kind":"advance"}));
            }
        }
    }
    println!(
        "{}",
        json!({"valid":true,"checksum":format!("{checksum:016x}"),"actor":state.input_actor().map(|s|s.index()),"phase":format!("{:?}",state.action_phase()),"remaining":state.supply().remaining_count(),"candidates":state.supply().candidates(),"neutral":state.supply().neutral(),"players":([Seat::First,Seat::Second].map(|s|{let p=state.player(s);json!({"buttons":p.buttons(),"income":p.income(),"time":p.time_position(),"occupied":p.board().occupied_count()})})),"bonus":state.bonus().owner(),"result":state.result(),"legal":legal})
    );
    Ok(())
}
