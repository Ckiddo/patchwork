# T36：拼布几何与安全落点

更新：2026-09-13。共享实现位于 [geometry.rs](../game_core/src/geometry.rs)，供原生服务端和 WASM 前端使用，不依赖 Bevy、网络或数据库。此阶段提供几何和状态上下文检查；T37 已将完整购买、时间收益与唯一奖励颁发组合成原子行动，见 [ACTIONS.md](ACTIONS.md)。T38 已接入服务端，T39 接入界面。

## 坐标与变换约定

- 图版坐标为 x 向右、y 向下，范围均为 0–8。BoardPosition 构造及 JSON 反序列化均检查范围。
- Orientation 保存 quarter_turns（0–3，顺时针每次 90°）和 flipped（基础形状水平翻折）。非法次数直接拒绝，不对外部输入取模。
- 按 **基础形状左右翻折 → 顺时针旋转 → 平移归一化** 执行。旋转使用 `(x, y) → (-y, x)`；归一化后最小 x 和最小 y 都为 0，格子按 `(y, x)` 排序。
- anchor 始终表示变换后包围盒的左上角。该格允许恰好是形状缺口；旋转和翻折都不修改 anchor。
- rotate_clockwise / rotate_counterclockwise 只改变合法姿态；flip 切换基础翻折标记。F 的语义是切换基础翻面，不是沿当前屏幕方向再次镜像。
- distinct_orientations 返回至多八种不同形状，对称姿态去重。例如 1×2 只有横/竖两种几何；非对称拼布可有八种。
- 特殊 1×1 使用轨道位置作为身份，只接受 `(quarter_turns=0, flipped=false)`，与 T35 保存格式一致。

旧 Bevy 演示的世界坐标 y 向上；T39 需在渲染边界做 y 轴映射，不能直接把旧 ShapeDirection 的计算当成正式几何。冻结规则定义不随显示坐标改变。

## 接口与校验层次

| 接口 | 作用 |
|---|---|
| patch_shape(PatchId, Orientation) | 安全查找冻结定义，返回归一化格子和包围盒尺寸；ID 不作为数组下标 |
| PatchShape::cells_at(anchor) | 得到图版坐标，任何一格越界则整体失败 |
| QuiltBoard::preview_placement(piece, anchor, orientation) | 检查形状、边界、重叠和同一拼布重复放置；只读生成图版副本和 PlacedPiece |
| GameSnapshot::preview_placement(authenticated_user_id, request) | 在几何检查前验证玩家身份、自己图版、Running 状态、当前行动者、普通候选或特殊队首 |
| QuiltBoard::completed_bonus_square() | 返回第一个连续满 7×7 的左上角，无则 None；先按行再按列查找 |
| PlacementPreview::completed_square() | 在预览落子后的图版上检查 7×7，普通拼布和特殊拼布共用 |

PlacementRequest 保存 target_seat、piece、anchor、orientation，不能携带新的占格矩阵。适配已有 Protobuf 动作时，坐标和 quarter_turns 必须经过检查后构造这些类型；不要先用 `as u8` 截断非法数值。authenticated_user_id 必须由服务端会话提供，不能相信动作中自报的玩家身份。

```rust
use game_core::{BoardPosition, Seat};
use game_core::geometry::{Orientation, PlacementRequest};
use game_core::rules::PatchId;
use game_core::state::PieceId;

let request = PlacementRequest {
    target_seat: Seat::Second,
    piece: PieceId::Normal(PatchId(10)),
    anchor: BoardPosition::new(3, 4)?,
    orientation: Orientation::new(1, false)?,
};
let preview = snapshot.preview_placement(authenticated_user_id, request)?;
// 渲染 preview.placed_piece().cells()；snapshot 保持原样。
```

错误明确区分：未知拼布、非法姿态、越界、重叠、重复放置、未知玩家、对手图版、非运行状态、非当前玩家、错误行动阶段、已取走和非候选。特殊阶段仅允许当前队首领取者放置对应轨道位置的 1×1，不能跳队、抢对方拼布或提前购买普通布。

**成功的预览只说明几何和上述状态条件满足，不代表购买已经合法完成。** T37 的 apply_action 会重新检查当前输入状态及余额，并同时计算取布、扣费、收入、时间、特殊队列和奖励；T38 提交时校验最新版本。预览不改变任何正式状态，也不是可以绕过重新校验的提交凭证。

## 7×7 与快照恢复

9×9 图版共有 9 个可能的 7×7 左上角，逐一检查其中 49 格全部已占用。普通和特殊拼布同等计入占格，不按拼布数量或总面积估算。49 格但不连续不能达成；80 格只缺中央一格也不能达成。

GameSnapshot::validate 和反序列化现在会按保存的姿态重建每块拼布，比较完整格子集合。格子记录顺序可以不同，但同面积的错误形状、错误旋转/翻折和断开的假形状均拒绝。

奖励归属也要求空间依据：已有 owner 时，该玩家必须有完整 7×7；存在满 7×7 却没有 owner 的正式快照不能恢复。T37 在落子事务内颁奖后再验证最终状态。双方都有满区时，仅凭最终占格无法判断谁先完成，先后顺序仍由行动引擎和持久化事件保证；不能依靠几何重新分配奖励。

## 旧演示修复与阶段边界

旧 BoardGame::can_put 已改为安全 get 查找，拒绝 idx == len、缺失位置记录和非法锚点；BoardGame::put 也先检查再写入，防止直接调用留下部分占格。原演示共享占格、点击左图版映射到右侧、旧旋转绘制等路径仍由 T39 整体替换，不作为正式规则入口。

T36 本身没有新增数据库迁移；T38 已启用 patchwork_custom_v1 正式开局和服务端落子，见 [权威对局](AUTHORITATIVE_GAMEPLAY.md)。按钮、键盘旋转和正式联机画面由 T39 接通，历史空对局继续兼容。

## 验收

新增 14 项测试，覆盖手工列出的八种非对称姿态、四次旋转/两次翻折复原、所有 33 块 × 8 姿态 × 81 锚点的边界、重叠和重复放置、身份与阶段限制、普通/特殊补齐所有 7×7 锚点、非连续面积、快照形状损坏和全部姿态恢复。成功和失败预览均检查输入状态不变。

本轮 50 项普通 Rust 测试、严格 Clippy、核心格式检查和 patchwork/game_core 的 WASM 编译通过。WASM 前端有 4 项原有演示代码警告。本次未改数据库或网络运行路径，37 项需独立 PostgreSQL 的集成测试未重跑；此次命令中显示 ignored，不计入 50 项通过。日志位于 artifacts/t36-rust-tests.log、artifacts/t36-clippy.log、artifacts/t36-wasm.log。
