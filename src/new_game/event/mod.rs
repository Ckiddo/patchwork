use bevy::prelude::*;

#[derive(Event)]
pub struct PatchChoosedEvent{
    pub patch_idx: usize,
}
pub fn observe_patch_choose_event(e: On<PatchChoosedEvent>){
    // 管理选中后的渲染

}