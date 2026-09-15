# T38 权威服务端、事务与恢复

更新：2026-09-13。正式规则 `patchwork_custom_v1` 已接入现有好友房、Actix Actor、Protobuf WebSocket 与 PostgreSQL。规则计算使用 [game_core](../game_core/src/actions.rs)，权威入口位于 [gameplay.rs](../backend/src/persistence/gameplay.rs)。客户端发送动作意图，服务端决定操作者、落点是否合法、费用、收益、行动顺序与结果。

T39 页面已默认创建 `patchwork_custom_v1`，通过 Bevy 场景与 Yew 操作栏提交正式动作，见 [前端交互](FRONTEND_GAMEPLAY.md)。T40 继续覆盖完整双浏览器边界与故障场景。T38 已支持通过现有 CreateRoom/SetRules 选择正式规则，双方重新准备后 Start。历史 `friend_room_empty_v1` 不自动转换。

## 真实开局与版本

Start 保留原来的房主、双方在线及准备校验。第一次有效提交使用服务端随机排列初始化 33 块供给，保存实际排列、座位、先手与完整 `GameSnapshot`；幂等重试先读原回执，重启和刷新都不再次随机。数据库事务若回滚，未提交的候选状态不会广播。

正式快照 `kind=patchwork_game_v1`、`schema_version=1`。JSON schema、规则版本、数据库 state_version、event_seq 和 Protobuf 版本各自独立；此次无需数据库迁移。读取正式快照时验证核心全部不变量，同时核对 game_id、数据库玩家顺序和生命周期投影。

旧通用 `begin_game`/`write_game` 仅保留演示仓储兼容路径，拒绝正式规则，防止调用者通过提交整份 JSON 绕过权威初始化或行动判定。

## 协议与授权

沿用 `GameRequest` 的 `advance`、`buy_and_place`、`place_special_patch`、`resign`，没有新增同义命令或更改字段编号。

- 操作者来自认证后的连接许可；取得数据库锁后再次核对 session、持久 connection_generation 和可撤销许可。新连接必须先完成 Resume/SyncAck。
- 普通行动要求双方在线、都已同步该对局，并检查快照中的行动者与普通/特殊放置阶段。认输允许已同步的玩家在暂停期间执行，不要求对手在线或轮到自己。
- 位置必须存在，x/y 在 0～8；购买位置为旋转翻折后包围盒左上角。Protobuf 坐标为 `sint32`，旋转次数必须为 0～3，不能把溢出值截断。拼布 ID 使用规范十进制字符串。
- `expected_version` 使用游戏版本。越界、重叠、轮次错误、余额不足和阶段不符都返回确定错误，不修改状态。

新增错误码，原有枚举值不变：

| 编号 | 名称 | 含义 |
|---|---|---|
| 22 | GAME_NOT_RUNNING | 对局暂停或已结束 |
| 23 | NOT_YOUR_TURN | 当前不是该玩家行动 |
| 24 | INVALID_PLACEMENT | 落点越界、重叠或重复落子 |
| 25 | INSUFFICIENT_BUTTONS | 购买前余额不足 |
| 26 | WRONG_ACTION_PHASE | 动作与普通/特殊放置阶段不符 |

既有 SYNC_REQUIRED、VERSION_CONFLICT、REQUEST_ID_CONFLICT、FORBIDDEN 等继续使用。传输和兼容基线见 [PROTOCOL.md](PROTOCOL.md)。

## 一次行动的事务边界

`SessionRegistry → LobbyManager → Room → friend_mutate → gameplay::execute` 复用同一房间串行队列。普通房间命令、游戏动作和连接观察不会开设互不协调的队列。锁顺序仍为 UUID 排序的用户 → 占用 → 房间 → 游戏。

1. 验证身份并读取 `(game_id,user_id,request_id)` 回执。同一 ID、同一意图返回原回执；同一 ID、不同意图拒绝。指纹覆盖用户、game_id、expected_version 和动作内容，不含连接代次，因此新连接同步后可恢复原请求。
2. 无回执才检查最新版本和规则前提，并通过纯核心计算完整 transition。客户端不提交余额、收入、时间、领取者或终局分数。
3. 同一 PostgreSQL 事务更新游戏快照、版本和事件序号，写入事件批次、动作回执并递增房间版本。自然终局或认输同时写唯一 game_results 和房间 Finished。
4. COMMIT 确认成功后才发布最新已提交快照。旧请求的回复保留原版本，广播读取当前版本，避免历史回执覆盖较新状态。

一次成功行动只增加一次游戏版本和一个事件序号，即使跨过多个轨道标记。费用、移动收益、收入、特殊拼布领取/队列、落子、7 分奖励和终局都来自同一次核心计算。结果分数为实际余额加单独奖励；写入 PostgreSQL integer 时检查转换，不截断。

COMMIT 响应丢失时沿用 Room 的未知提交处理：保留队列和用户预留，按相同锁顺序查询回执。查到回执恢复原结果；确认没有回执才允许原请求重试；数据库仍不可用时继续保留未决状态。自然终局的回执与唯一结果也在这个事务中。

## 快照、事件与恢复

正式对局使用 `GameEvent.payload_json` 中的 `game_transition_v1`，包含 game_id、rules_version、source、phase、按顺序的 ActionEvent 列表和该次提交后的完整 state。外层 GameEvent 保存 seq/version。事件内完整快照用于确定性恢复，事实列表可供之后的动画和账目展示；客户端恢复不会重新发放收入或重放扣款。

连接暂停/恢复同样写正式核心快照和事件；领取队列、图版、资源、初始排列、中立指示物、上一普通行动者与奖励保持原值。服务重启先恢复为 Paused，双方完成同步后恢复 Running。核心 `with_connection_pause` 负责生命周期变更；`terminate` 处理认输、掉线弃权和双方离线废弃，保留实际分数。自然计分仍由行动引擎完成。

数据库结果原因分别为 completed、resigned、timeout、abandoned；核心结果中认输与掉线统一为 Forfeit，后端结果原因保留两者区别。双方离线废弃为 Abandoned，无胜者，不冒充等分平局。

Resume 继续在一致的已提交视图下返回至多 64 条连续事件；缺口、缺少客户端快照或总消息超过 64 KiB 时返回全量快照。浏览器读取器支持历史 `connection_state_v1` 和正式 `game_transition_v1`，检查连续 seq/version、最终游标及核心快照，再发送 SyncAck；未知或损坏内容退回全量同步。

连接恢复预算、心跳与背压仍遵守 [CONNECTION_RECOVERY.md](CONNECTION_RECOVERY.md)。数据库故障和服务器停机时间不作为玩家正常在线期间的恢复预算扣除。

## 复现测试

以下命令在项目根目录执行；PostgreSQL runner 只建立本项目 artifacts 下的临时 loopback 集群，结束后停止清理。

```powershell
cargo test --locked --offline -p backend -p util_lib -p game_core
cargo clippy --locked --offline -p backend -p util_lib -p game_core --all-targets -- -D warnings
cargo fmt -p backend -p util_lib -p game_core -- --check
cargo check --locked --offline -p patchwork -p game_core --target wasm32-unknown-unknown
cargo build --locked --offline -p backend
& D:\Tools\Python\python.exe tools/ci/postgres_suite.py --bin-dir D:\Tools\PostgreSQL\18.6\pgsql\bin
```

事务与规则测试位于 [gameplay.rs](../backend/tests/support/gameplay.rs)，实际双 WebSocket 与强制进程重启脚本位于 [gameplay_smoke.py](../tools/ci/gameplay_smoke.py)。执行结果和阶段完成状态见 [开发 TODO](DEVELOPMENT_TODO.md)。
