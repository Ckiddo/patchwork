use bevy::prelude::*;
use crate::new_game::{game_state::InteractiveInfo, patches::ShapeChooseMark};

#[derive(Event)]
pub struct PatchChoosedEvent {
    pub patch_idx: usize,
}
pub fn observe_patch_choose_event(
    e: On<PatchChoosedEvent>,
    mut query: Query<(&mut Visibility, &ShapeChooseMark)>,
    mut int_r: ResMut<InteractiveInfo>,
) {
    // 管理选中后的渲染
    for (mut v, s) in query.iter_mut() {
        if s.patch_idx == e.patch_idx {
            *v = Visibility::Visible;
        } else {
            *v = Visibility::Hidden;
        }
    }

    // 选中状态做标记
    int_r.choosing_shape = Some(e.patch_idx);
}
