# 断线、恢复与背压

2026-09-13，T25–T29，随 T38 更新。恢复流程已接入好友房、PostgreSQL、WebSocket 和浏览器，支持历史空对局与正式 patchwork_custom_v1。本文说明身份、座位、恢复预算和握手；完整动作与结果持久化见 [权威对局](AUTHORITATIVE_GAMEPLAY.md)。

## 连接与房间生命周期

`SessionRegistry` 只承认一个 `user_id/session_id/generation`。注册使用非阻塞 Actor future，同一用户最多一个注册在途，全局最多 128 个；数据库在递增 generation 的事务中重新检查持久会话。旧连接的消息、清理回调和已排队的房间变更都受代次限制。注册提交确认丢失时撤销旧连接许可，避免旧内存代次长期停留在已被数据库替代的状态。

WebSocket 使用统一 Lease 清理入口。Close、流结束、协议错误、认证/访问令牌/心跳超时、发送失败及任务被取消都经过同一个幂等 Disconnect；清理只有在 stamp 匹配时才移除连接。认证中途结束也不会留下永久连接。Registry 每秒清除已经失效、过期或失去接收方的许可，覆盖认证回复尚未交付时的竞态。

每个 Room Actor 每秒安排一次连接观察，与房间变更共用串行执行入口。正在处理业务或未决提交时合并定时任务，不积压计时消息。观察先取 Registry 的在线与同步状态，再按用户 UUID、占用、房间、对局顺序锁定数据；准备中的接管、代次不一致或观察过程中连接状态变化都会使本轮回滚。提交之后才更新 Lobby 索引和向成员推送。空房 Actor 退出，活动房间保留恢复计时。

## 默认策略

配置见 `backend/config.example.toml` 的 `[recovery]`，以下四项可设为 1～86400 秒。

| 配置 | 默认值 | 行为 |
|---|---:|---|
| waiting_grace_secs | 60 秒 | 等待房及已结束房保留离线座位，超时释放占用；房主转给剩余座位，最后一人移除后关闭房间 |
| game_budget_secs | 每人累计 120 秒 | 对局中单人离线/未完成同步时累计；重连不重置余额，仅在对手在线且已确认状态时扣除和判定超时 |
| both_offline_retention_secs | 600 秒 | 双方未恢复时暂停单人预算，持续超过保留期后 Abandoned，无胜者 |
| restart_grace_secs | 120 秒 | 重启时活动对局置为 Paused，给予相同恢复宽限；双方恢复并确认后可提前结束宽限 |

等待房的离线或重新接入会清空准备共识。座位不会重排，先手也不会重新随机。两人都掉线后，有一人恢复时重新开始观察单人计时，不把上一段双方离线时间算到另一人头上。建立 WebSocket 但一直不完成 SyncAck，也不会无限延长个人预算。

这些时间是**服务健康时累计的观察时间**，不是直接比较墙上时钟。观察间隔超过 3 秒、单次观察耗时超过 2 秒、锁/连接池/数据库错误或连接拓扑变化时不扣这一段；错误后的第一轮从零间隔重新采样。服务重启不计算停机时间，并保留已经用掉的个人预算。这是偏保守的 P0 策略，服务繁忙可能延长实际宽限，避免因服务故障判玩家负局。100 连接/50 房间的容量目标仍需 T53 压测。

## 持久化与提交结果未知

迁移 `0006_connection_recovery.sql` 新增：

- `room_members.offline_ms`：当前等待座位的连续离线计时；`disconnected_at` 保存观测到的离线时间。
- `room_recovery`：服务启动 epoch、最后 tick UUID、两人的累计消耗、两人的恢复状态、双方离线时间和重启剩余宽限。
- `game_events.user_id` 允许 NULL，代表服务端生成的生命周期事件；玩家事件仍记录操作者。

每轮观察保存唯一 tick UUID。COMMIT 确认丢失后用相同 UUID 重新加锁查询；已经提交就返回当前状态，不再次扣费或结算。恢复到旧 epoch 的 Actor 无权继续计时。重启初始化也按 epoch 幂等，重复初始化不会不断延长宽限。

暂停、恢复、超时结束与无胜者结束，都在事务中递增对局版本/事件序号。历史空对局仍写 connection_state_v1；正式规则写 game_transition_v1 并更新核心 lifecycle，保留待放队列、图版与资源。终局同时保存唯一 game_results 并更新房间为 Finished；事件和结果不会在提交前推送。正式对局记录实际余额加奖励，历史空对局保留原来的 0 分占位结果。

## Resume 与同步确认

1. 新连接首先完成 Authenticate，保留原 user_id。Get 房间和待确认的原请求用于恢复房间上下文。
2. 客户端发送 `ResumeRequest(game_id,last_seq,has_snapshot)`。服务端校验持久会话、连接代次及该对局玩家身份，在房间共享锁下读取同一已提交快照与事件尾部。
3. 客户端确有已有快照、尾部连续且最多 64 条时返回事件；序号缺口、事件被清理、没有本地快照或尾部过大时回退全量 GameSnapshot。未来序号拒绝，版本/序号全程使用 u64。
4. ResumeState 带当前版本、末尾序号、phase 和本连接的随机 sync_token。客户端安装快照或按顺序应用事件后发送 `SyncAck`。
5. 服务端重新核对当前持久版本和该连接保存的 token。版本在握手期间改变就重新同步；旧连接或旧 token 不能解锁新连接。确认前 Game 请求返回 `SYNC_REQUIRED`。双方都同步后可执行正式规则动作；已同步玩家也可在暂停时认输。旧空对局不执行正式动作。

浏览器读取 connection_state_v1 和 game_transition_v1 的已提交状态，校验事件序号/版本连续、最终游标和核心快照，不认识或损坏的事件回退全量快照。恢复不重新计算费用、收入或领取特殊拼布。旧客户端不发送新增字段时仍能解码既有 v1 消息，但需要升级才能完成对局恢复握手。

## 心跳与背压

| 边界 | 上限/策略 |
|---|---|
| 未认证连接 | 首帧认证总时限默认 5 秒；仅接受二进制 Authenticate |
| 应用心跳 | 浏览器每 15 秒 Ping；45 秒没有 Pong 主动重连；服务端 45 秒无有效应用输入清理 |
| WebSocket 输入 | 单帧 16 KiB；不支持分片业务消息；每秒 20 条、突发 40 条，控制帧同样计数 |
| WebSocket 输出 | 单条 64 KiB，每连接推送队列 32 条，每次发送等待最多 1 秒 |
| 浏览器发送缓存 | bufferedAmount 超过 64 KiB 或输入超过 16 KiB 时断开并恢复 |
| 并发工作 | 每连接一条业务操作在途，后续业务返回可重试 PlayerBusy；心跳仍独立处理 |
| Actor 队列 | Registry/Lobby 邮箱 128，Lobby 用户预留最多 128；Room 邮箱和业务队列各 32 |
| 总连接 | 最多 1024 条已升级 WebSocket，包含认证阶段；注册在途最多 128 |

常规推送经有容量限制的 Actor send，再使用单连接 try_send；慢客户端的队列满会立即撤销其许可，不拖住其他收件人。清理消息使用可靠 do_send，其数量受连接总量约束。等待数据库响应时 WebSocket 仍接收心跳，出站推送不会重置输入存活期限。协议限制会保守关闭连接，已提交的原请求通过回执恢复。

网络断开和访问令牌到期采用 0.5 秒起、最大 30 秒的指数退避加抖动，先恢复原身份再连接；房间操作记录保留。被另一标签页接管、注销或策略关闭（1008）不会自动争抢连接。界面在断线和同步期间禁用操作，恢复后显示对局 phase、版本和事件序号。

## 本机验证与复现

使用现有 `D:\Tools\PostgreSQL\18.6\pgsql`，测试数据、随机凭据和日志放在项目 `artifacts/postgres-test-*`，测试结束停止并清理。没有在 E 盘复制程序或初始化常驻库。

```powershell
cargo test --locked -p backend -p util_lib -p game_core
cargo clippy --locked -p backend -p util_lib -p game_core --all-targets -- -D warnings
node --test tools/ci/browser_session.test.mjs
cargo build --locked -p backend
D:\Tools\Python\python.exe tools/ci/postgres_suite.py --bin-dir D:\Tools\PostgreSQL\18.6\pgsql\bin
```

PostgreSQL 回归目前 36 项，恢复新增 9 项：等待宽限/旧代次/座位与房主、累计预算与双方离线切换、无胜者结束、重启与过时采样、补事件/缺口回退/ACK、慢接收者隔离、tick 提交确认丢失、暂停数据库响应/注册不阻塞 Registry，以及真实 Room 定时器与 Lobby 目录更新。数据库暂停用测试协议代理暂停响应模拟，并未暂停或修改其他 PostgreSQL 实例。

`tools/ci/recovery_smoke.py` 使用真实二进制与 WebSocket，强制结束并重启测试后端，验证原对局、座位、已提交状态和原开局回执，覆盖恢复确认、接管、消息/速率限制以及持续出站推送下的 45 秒心跳超时。Node 测试共 10 项，新增 Pong 存活、持续推送不能替代 Pong、发送缓存限制与接管不重试。结果日志为 `artifacts/recovery-postgres.log`、`artifacts/backend-smoke.json`、`artifacts/postgres-suite.json`。

前端可用隔离预览测试实际重连：

```powershell
$env:PATCHWORK_API_BASE='http://127.0.0.1:8000/api'
trunk build --release --dist artifacts/browser-dist
D:\Tools\Python\python.exe tools/ci/postgres_suite.py --bin-dir D:\Tools\PostgreSQL\18.6\pgsql\bin --preview-dir artifacts/browser-dist --preview-port 8081
# 两个来源使用独立身份：http://127.0.0.1:8081 与 http://localhost:8081
New-Item artifacts/browser-preview.restart -ItemType File
# 上一行只强制重启该 runner 拥有的临时后端；页面和数据库继续运行。
New-Item artifacts/browser-preview.stop -ItemType File
```

正式发布前要用迁移账号先执行 0006，再启动匹配的后端与前端版本。目标仍为 `192.168.5.9:D:\deploy_patchwork`，Cloudflare 仅中转；本阶段没有连接远端、配置 Tunnel、提交或推送。公网中转、生产部署、Linux/GitHub Actions 和负载测试另行验收。

2026-09-13 实际双浏览器验收：两个来源建立独立身份，双方开局完成 SyncAck。刷新房主页面后身份、座位、先手及 game_id 不变；随后强制结束临时后端并重启，两页无需手动刷新自动恢复原对局，双方显示已确认的版本 2 / 事件序号 2，等待/恢复提示与布局已目视检查。记录：`artifacts/recovery-browser.json`。两个页面、临时后端/静态服务及临时 PostgreSQL 集群已停止清理。
