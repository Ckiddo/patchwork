# T35：玩家、供给与时间状态

更新：2026-09-13。在 T34 冻结数据之上实现纯 Rust 可变状态与结构校验。源码入口：[state/mod.rs](../game_core/src/state/mod.rs)、[state/supply.rs](../game_core/src/state/supply.rs)，测试见 [state/tests.rs](../game_core/src/state/tests.rs)。

## 状态结构

| 类型 | 保存的内容 |
|---|---|
| GameSnapshot | game_id、规则/JSON 格式版本、先手、双方玩家、供给、特殊拼布、行动记录、奖励、生命周期和结果 |
| PlayerState | user_id、固定座位、独立图版、已放拼布、纽扣、收入、时间位置 |
| QuiltBoard | 9×9 占格；每格关联普通 PatchId 或特殊拼布轨道位置 |
| PlacedPiece | 拼布身份、占格坐标、旋转次数和翻折标记；收入按普通拼布计一次 |
| SupplyState | 初始 33 块排列、同位置剩余槽位、中立指示物 |
| SpecialPatchState | 轨道位置及 Available/Pending/Placed/Discarded 状态，保存领取者、落点或弃置原因 |
| ActionState | 上一名普通行动者、按轨道顺序排列的待放特殊拼布队列 |
| BonusState | 唯一 7×7 奖励获得者，None 或一个座位 |
| GameResult | 双方纽扣/奖励/总分、自然计分/弃权/废弃原因、胜者或平局/废弃结果 |

图版数据按玩家独立保存，不再共享一份占格；核心中没有 bank_money、Bevy Entity、数据库句柄或连接计时器。旧演示的渲染数据留在原处，由 T39 替换，当前页面还没有使用这份正式状态。

## 初始化与身份映射

```rust
use game_core::{Seat, rules::PATCHES, state::GameSnapshot};

let order = PATCHES.iter().map(|p| p.id).collect(); // 示例固定排列；实际由服务端洗牌后传入
let snapshot = GameSnapshot::new(
    "game-1".into(),
    ["player-a".into(), "player-b".into()], // 实际接入传数据库 UUID 字符串
    Seat::Second,
    order,
)?;
```

只接受完整、不重复的 33 块排列；不存在的 ID、缺少拼布、重复身份或非法身份字符串均拒绝。核心不生成随机数，同一输入得到相同状态。每名玩家初始化 5 纽扣、0 收入、时间 0 和空图版。

Seat 的 JSON 表示固定为 0/1，拒绝其他值，保持与现有房间 first_player_seat 投影兼容。snapshot.perspective(user_id) 返回 (自己, 对手)，前端将它映射到 (右, 左)；房主身份不影响视角。

## 供给和中立指示物

- 初始排列保留 33 个固定槽位；取走拼布后留下 None，不移动其他槽位。
- BeforeSlot 表示初始在 1×2 前面，从该槽位开始找候选。
- OnVacatedSlot 表示取布后处于被取走拼布的空槽，从下一槽位开始找候选。
- candidates 最多环绕一次、跳过空槽、取至多三块。剩余 2/1/0 块时不会重复列出同一拼布。
- take_candidate 只执行供给状态变化；无效候选不改变输入状态。T37 在同一份候选新快照上组合扣费、合法落子和时间推进，不能把这个低层方法单独当成完整购买行动。

初始候选为 10/11/12 时，取走 11 后变为 12/13/14，10 保留。测试遍历所有 33 个中立起始槽位并耗尽供给，验证环尾与不足三块情况。

## 行动与连接暂停分开

当前行动者从状态推导，不重复保存一份可能过期的 current_player：

1. 已有结果 → Finished。
2. 待放特殊拼布队列非空 → 队首领取者放置对应特殊拼布。
3. 双方都在终点且没有待放特殊拼布 → AwaitingScoring，禁止继续普通行动。
4. 时间较小者行动；同格取上一普通行动者的另一方。
5. 开局没有上一普通行动者时，使用已保存的先手。

Lifecycle 保存 Running/Paused/Finished。暂停不会清空队列或重置回合；input_actor 在暂停和终局时返回 None，action_phase 仍能说明恢复后应该处理什么。后端断线预算、连接代次和重连确认继续由原有网络层负责，T38 已通过核心生命周期接口适配。

特殊拼布放置不改 last_normal_actor。一次越过多个特殊拼布时，队列必须与领取状态一致、按轨道顺序处理，且归本次普通行动者；待放阶段即使双方已到终点，也要先完成放置再进入计分。

## 序列化与恢复边界

GameSnapshot 实现 Serialize 和经过 validate 的 Deserialize。正式快照使用 kind=patchwork_game_v1、rules_version=patchwork_custom_v1、schema_version=1；state_version、事件 seq 和连接恢复 token 继续放在现有外层协议/仓储中。

恢复会检查：

- 格式版本、身份唯一性、固定座位、时间范围和棋盘坐标。
- 完整初始排列、合法中立位置、剩余拼布与初始槽位对应。
- 普通拼布在“剩余供给 + 双方已放”之间恰好出现一次。
- 已放拼布的完整形状与保存的旋转/翻折一致，格子不重复/不重叠、占格矩阵和放置记录一致；普通拼布收入总额一致。
- 已有奖励归属必须对应该玩家的连续满 7×7；存在完整区域却未记录奖励归属的正式快照也拒绝恢复。
- 特殊拼布坐标、领取者、时间位置、已放记录和待放队列一致；不能恢复已消耗的公共拼布。
- 满图版弃置必须有满图版依据；终局分数明细、结果原因与胜者一致，自然计分必须已到终点且队列为空。

T36 已补齐变换后的完整形状与连续 7×7 校验，并提供只读安全落点预览，见 [GEOMETRY.md](GEOMETRY.md)。T37 已实现费用、时间收益、领取/放置、奖励颁发和自然终局，见 [ACTIONS.md](ACTIONS.md)。新行动通过 apply_action 计算后校验最终快照；恢复校验仍不能独立证明某份终态的完整历史，可信初始状态、行动序列和事务由 T38 维护。

所有可变字段只在核心 crate 内部开放给行动引擎，客户端只读取或提交动作意图。T38 已开放正式规则的服务端开局、动作和持久化，schema 保持 1；读取时额外核对数据库 game_id、player0/player1 和 phase。完整事务与恢复见 [AUTHORITATIVE_GAMEPLAY.md](AUTHORITATIVE_GAMEPLAY.md)。

## 验收

新增 12 项状态测试：确定性开局、非法身份/排列、候选环绕与无效取布不变、全部起始槽位耗尽、非法供给恢复、双方状态隔离、连续行动/同格、待放队列与暂停恢复、终局前特殊拼布、损坏快照、领取归属和弃权/废弃结果。

完整回归结果记录于 [开发 TODO](DEVELOPMENT_TODO.md)。本地手动联调使用 [一键测试说明](LOCAL_TEST.md)。T36–T38 的共享几何、完整行动、权威事务与恢复已完成，下一步为 T39 联机棋盘交互。
