# 好友房间

2026-09-13，T18–T24，随 T38 更新。好友房已接入 PostgreSQL、Actix Actor、Protobuf WebSocket 和 Yew 页面；恢复见 [断线恢复](CONNECTION_RECOVERY.md)，正式规则开局和动作事务见 [权威对局](AUTHORITATIVE_GAMEPLAY.md)。页面默认创建 patchwork_custom_v1，T39 已接入 [Bevy 联机棋盘](FRONTEND_GAMEPLAY.md)。历史空对局明确带有 rules_implemented=false；FIFO 匹配暂缓。

## 对外行为

通过 `/api/ws` 完成首帧认证后发送 `ClientEnvelope.lobby`。操作者、会话和连接代次均来自认证上下文，客户端不能指定操作者。

| 命令 | 条件和结果 |
|---|---|
| Create | 当前玩家空闲；mode 为 casual，提供规则版本标识和可选密码；返回完整房间快照 |
| Join | 通过房间 ID/短码加入，仅 Waiting 且有空座位；同时提供 ID/短码时必须一致；加入后双方准备清零 |
| Leave | Waiting/Finished 中允许成员退出；房主退出后转移给剩余成员，座位不变；最后一人退出转为 Closed |
| SetReady | 设置明确的 true/false，校验成员、Waiting 和 expected_version；重复请求不反转状态 |
| SetRules | 仅房主在 Waiting 修改规则标识；规则实际变化时双方准备清零 |
| Start | 仅房主；恰好两个不同用户、座位为 0/1、均准备且连接有效；创建唯一 game_id 并向双方推送同一初始快照 |
| List | 按 room_id 升序的 keyset 分页，只列 Waiting；limit 为 1–50，next_cursor 为下一页游标 |
| Get | 仅成员获取完整房间与当前对局；省略 room_id 时查询自身占用的房间 |

UI 提供创建、密码、短码加入、列表/下一页、双座位、准备/取消、房主开局、离开、规则修改和对局编号。桌面端采用紧凑身份栏、左侧好友房面板和右侧游戏画布；规则修改与版本信息位于可展开的“房间设置与版本”。原 Bevy 画布保留为“本地规则演示”。

页面按浏览器可视高度分配空间，画布随剩余区域调整尺寸，相机按比例容纳完整的 1920 × 1080 游戏区域。常规桌面窗口中，房间操作与棋盘同时可见，无需滚动整页；过长的房间列表、展开设置或窄屏下的溢出只在房间面板内滚动。宽度不超过 600 像素时改为上下分区，画布仍保留独立空间。

请求与错误提示使用固定高度区域，刷新、准备等请求不会临时插入段落推移房间内容。等待超过 250 毫秒才显示请求提示和原请求重试按钮；等待期间保留禁用与幂等保护，按钮不再整体变淡。离线与房间已满等持续不可用状态仍正常显示为禁用。心跳响应只更新连接存活时间，不触发房间重绘。

规则必须在注册表中：v1、friend_room_empty_v1 保留空对局，patchwork_custom_v1 创建完整正式对局。创建、改规则和开局都检查受支持版本；未知字符串不能开局，旧空对局不自动转换。

## 版本、短码与幂等

- 变更命令带 expected_version。短码单独加入使用 expected_version=0，服务端先解析短码并读取当前版本，再按该版本执行条件事务；此后房间变化仍返回 VERSION_CONFLICT。列表加入带明确 room_id 和所见版本。
- 创建版本为 0；加入、离开、显式准备、规则命令和开局递增版本。相同 request_id/请求返回原回执，不重复递增；同 ID 不同请求返回 REQUEST_ID_CONFLICT。
- 短码为服务端生成的 10 位字母/数字串，数据库唯一约束防冲突。创建候选 ID/短码由服务器密钥、用户及请求 ID 确定，响应丢失或重启后可定位同一候选。
- 所有变更同事务保存 operation_receipts。请求指纹使用 HMAC，避免密码参与普通快速散列后成为离线猜测依据；回执和快照不保存明文密码。
- 指纹密钥从既有 JWT 签名密钥派生，需要持久保存；未来签名密钥轮换时应一并设计历史请求指纹兼容窗口。
- 重试返回原回执，但索引和广播读取最新数据库快照。前端按房间版本拒绝较旧快照覆盖新状态。

浏览器发送前将请求保存到按 API/用户隔离的 sessionStorage。未收到确定结果时保留原 ID，提供“重试原请求”，重新加载后先恢复该请求；确定的业务错误后才允许重新编辑。数据库、存储或网络失败不会触发创建新身份。

## Actor 与事务

`SessionRegistry → LobbyManager → Room → Persistence`：

1. Registry 检查当前 Stamp，将可撤销连接许可随命令传递。其自身不等待房间事务，心跳和接管可继续处理。
2. Lobby 单例维护恢复的用户占用索引、每房唯一 Actor 和带操作 ID 的用户预留。密码校验/短码解析等待期间也保留用户。
3. Room 的显式 pending 队列串行执行本房变更，不反向等待 Lobby。
4. 事务按 UUID 顺序锁用户，再锁玩家占用、房间；取得锁后重新核对成员集合、连接许可和持久 connection_generation，随后检查回执、版本、权限与业务条件。
5. PostgreSQL COMMIT 是成功边界。提交后同步当前快照、更新索引并广播，不提前发布未持久化状态。

COMMIT 确认丢失时，Room 冻结本房队列和 Lobby 用户预留，通过相同加锁顺序查询回执。有回执则恢复结果，确认无回执才返回可重试失败；查询仍失败则保留预留。邮箱关闭也先恢复查询，不能据此认定回滚。提交已确认但读取最新快照失败时，同样保持预留。

大厅索引记录已应用版本，旧回调不能覆盖新索引。阶段 5 起，活动 Room 保留每秒恢复观察，观察与业务工作串行；空房 Actor 退出，未决事务不会因闲置检查释放。数据库占用唯一约束仍是最终约束，内存预留不会替代它。

## 状态与空对局

`Waiting → Starting → Playing → Finished → Closed`；等待房最后一人退出也可直接 Closed。Starting、初始 game 写入和 Playing 转移在同一事务完成，外部不会观察到半个已开始房间。

Playing 禁止再加入、准备、修改规则和普通离房。正式对局由 gameplay::execute 原子保存自然终局/认输结果和房间 Finished，连接观察同样原子处理超时/废弃。通用 write_game 只保留旧演示兼容，拒绝写正式规则快照。

座位、房主、先手独立：房主转移不重排座位，新玩家填空座位，先手由服务端首次开局时选择并保存。所有成员接收相同 GameSnapshot，含玩家顺序、座位、规则版本、先手和 game_id。重复开局返回原回执，新 ID 对 Playing 再开局返回阶段错误。

排队期间已被接管的旧连接不能继续变更状态；迟到 Disconnect 无法删除新连接；开局不接受已知离线玩家。阶段 5 已实现断线清准备、座位宽限、累计恢复预算与暂停/补事件，重启时保留座位并递增房间版本、重置准备共识。

## 密码、容量与就绪

密码使用随机盐的 Argon2id PHC 散列，最多 128 个 UTF-8 字节；密码运算在事务外的阻塞任务中执行，限制两个并发任务。快照/列表只暴露 requires_password，错误密码返回 BAD_PASSWORD。[Argon2 官方实现说明](https://docs.rs/argon2/0.5.3/argon2/)

Lobby 邮箱/全局用户预留上限 128，每房 pending 队列与邮箱容量 32，每连接推送队列 32。推送队列满则结束该连接，不回滚已提交动作。阶段 5 已补 20 条/秒、突发 40 条的输入限制、单条 64 KiB 输出限制及单连接一条业务在途；负载实测仍属于 T53。

迁移 `0005_friend_rooms.sql` 增加 Waiting 房间按 UUID 分页索引。数据库健康、Lobby 可达且占用目录恢复后，`/readyz` 返回 200、rooms=ok；否则返回 503。它表示好友房可接受请求，不代表完整对战已经实现。

## 验证与复现

~~~powershell
cargo test --locked -p backend -p util_lib -p game_core
cargo clippy --locked -p backend -p util_lib -p game_core --all-targets -- -D warnings
node --test tools/ci/browser_session.test.mjs
cargo build --locked -p backend
D:\Tools\Python\python.exe tools/ci/postgres_suite.py --bin-dir D:\Tools\PostgreSQL\18.6\pgsql\bin
~~~

PostgreSQL 套件共 27 项，其中好友房新增 9 项：密码、人数/阶段/成员权限、准备与规则变化、房主转移、座位稳定、最后座位竞争、同用户两房、开局幂等/同一初始推送、Finished、重启恢复、旧回执、旧连接排队、失败事务不广播及 COMMIT 确认丢失恢复。原身份和数据访问测试保留。

`tools/ci/friends_smoke.py` 在真实临时后端上执行完整 WebSocket 好友房流程；runner 将其与原身份/生命周期测试一起运行。普通 Rust 回归 14 项，浏览器会话 Node 测试 8 项；Clippy、协议基线与 release WASM 打包也需通过。

2026-09-13 双浏览器本机验收：`127.0.0.1:8080` 与 `localhost:8080` 使用不同浏览器存储来源建立独立身份。实际点击建房、单人开局拒绝、按码加入、双方准备并开局；双方显示相同版本 4、共同对局和先手。房主重新加载后保持原身份/座位/房间/对局，已目视检查布局。脱敏记录：`artifacts/friends-browser.json`。

临时页面验收使用单独产物：

~~~powershell
$env:PATCHWORK_API_BASE='http://127.0.0.1:8000/api'
$env:NO_COLOR='true'
$env:CARGO_BUILD_JOBS='2'
trunk build --release --locked --public-url / --dist artifacts/browser-dist
D:\Tools\Python\python.exe tools/ci/postgres_suite.py --bin-dir D:\Tools\PostgreSQL\18.6\pgsql\bin --preview-dir artifacts/browser-dist
~~~

预览占用 loopback 8000/8080，端口须空闲，使用新的临时数据库。在另一终端创建 `artifacts/browser-preview.stop` 后会停止子进程并清理集群。重复使用预览需使用干净的测试浏览器存储，旧临时库身份不会自动替换。正式发布仍使用 HTTPS API 构建配置，不能发布该本机验收目录。

本次复用 `D:\Tools\PostgreSQL\18.6\pgsql`，没有复制到 E 盘、注册常驻数据库服务或连接部署主机。临时浏览器服务已停止。未提交、推送或部署；GitHub Actions/Linux 和公网 Cloudflare 链路仍待后续实际运行。
