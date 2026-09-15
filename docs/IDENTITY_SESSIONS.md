# 身份与浏览器会话

2026-09-13，T13–T17。身份资料与持久会话已接入 PostgreSQL，浏览器已接通首帧认证及心跳。后续阶段 4 已接入 [好友房间](FRIEND_ROOMS.md)；匹配、游戏动作和 Resume 仍待实现。

## HTTP 契约

所有动态响应保持 Cache-Control: no-store；令牌不放入 URL，错误不回显请求、令牌或数据库原始错误。

| 接口 | 行为 |
|---|---|
| POST `/api/auth/create` | 保留无请求体用法，返回原 jwt/identity，并新增 session_id/refresh_token。新版客户端提供预生成的 session_id/refresh_token，使创建请求可重试。 |
| POST `/api/auth/verify` | Bearer 认证；identity 从数据库读取，旧 JWT 内昵称不会覆盖资料。 |
| PUT `/api/auth/nickname` | 校验并更新数据库昵称，限制 1–20 个 Unicode 字符；保留 jwt/identity 响应。 |
| GET `/api/me` | 返回当前持久身份资料。 |
| POST `/api/auth/session` | 受控迁入旧 JWT，签发带持久 session_id 的新访问令牌。 |
| POST `/api/auth/refresh` | 校验并轮换恢复凭据，续期同一用户的访问令牌。 |
| POST `/api/auth/logout` | 撤销当前持久会话，关闭其有效游戏连接；该会话的访问及恢复凭据随即不可用。 |

访问令牌使用 HS256，带 sub、sid、auth_version，默认有效 15 分钟。每次 HTTP 认证查数据库的用户版本、会话过期与撤销状态。identity.created_at 始终是数据库用户建立时间，与 JWT 签发时间分开。数据库缺失/故障返回 503，不再签发临时身份。

新建持久会话后有效期为 30 天，成功刷新后延长 30 天。恢复凭据为 32 字节安全随机数的 64 位小写十六进制字符串，数据库只保存 SHA-256 散列。迁移 0004 新增上一轮散列、rotation_id、用户 legacy_exchanged 和 connection_generation；无需存储原始令牌或可逆加密的恢复凭据。

恢复请求体为 session_id、refresh_token、next_refresh_token、rotation_id。客户端先生成新的随机凭据及操作 ID，并把待执行请求原子保存到 localStorage，然后发送请求。服务端在用户行/会话行锁内轮换散列。相同旧凭据、相同下一凭据、相同 rotation_id 重试时，若当前散列仍是该下一凭据，则返回同一会话的新访问令牌；不同轮换请求竞争只有一方成功。重试校验同时需要旧凭据与当前新凭据，不是允许单独重放已消费的旧凭据。

创建请求同样使用预保存的客户端随机 session_id 和凭据；服务端在缺少会话行时使用事务 advisory lock 串行化，同一候选重试不会创建第二个用户。原无请求体创建方式为兼容旧客户端保留，不具备客户端候选丢响应恢复能力；新前端已使用有请求体方式。

## 旧 JWT 迁入

默认 `auth.allow_legacy_migration=false`。只有确认保留了旧 HS256 签名密钥时，才在项目配置中显式启用：

```toml
[auth]
allow_legacy_migration = true
websocket_auth_timeout_secs = 5
```

旧令牌必须验签成功、未过期、user_id 为合法 UUID、签发时间与昵称合法，且不能混入不完整的新会话字段。不能从未验证的 JWT 解码结果导入身份。第一次迁入保留旧 user_id 和 created_at；已有数据库昵称和建立时间优先。交换为持久会话后标记 legacy_exchanged，旧令牌不能再创建其他会话；相同客户端候选在交换响应丢失后仍可重试。

迁入窗口结束后关闭配置。没有旧密钥或旧令牌已经过期时，无法安全证明旧访客身份；新前端保留旧本地记录并报告错误，不会悄悄新建用户。访客跨设备找回和账号绑定不在本阶段。

## 浏览器状态与 API 地址

`src/browser_session.mjs` 管理访问/恢复凭据和持久待执行请求，`src/browser_session.rs` 连接 Yew 与共享 Protobuf。缓存按 API 地址隔离，保留旧 game_jwt_token 的受控迁入路径。

- 网络错误、超时、5xx、429、非 401 错误、JSON 解析错误：保留缓存和身份，不创建新用户。
- 验证返回 401：尝试同一会话刷新。刷新仍返回 401 时保留本地状态，等待用户处理；不自动替换身份。
- 使用 Web Locks 对同一 API 的初始化/刷新跨标签页串行化。缺少安全上下文或 Web Locks 时明确失败，不悄悄退化为存在竞争的刷新流程。[Web Locks 官方说明](https://developer.mozilla.org/en-US/docs/Web/API/Web_Locks_API)
- 使用一个 localStorage JSON 记录原子保存会话或 pending 请求。存储写入失败时不执行后续网络副作用；刷新或创建响应丢失后，重新加载复用原 pending 请求。
- 浏览器 Fetch 超时 12 秒。恢复凭据保存在浏览器本地存储，应用不输出它们；当前未使用跨站第三方 Cookie 方案。

开发默认 API 为 `http://127.0.0.1:8000/api`。正式 API 地址通过构建变量提供，必须指向最终 Cloudflare HTTPS 中转入口：

```powershell
# 在此子进程中设置实际已配置的 HTTPS API 根地址，必须以 /api 结尾。
$env:PATCHWORK_API_BASE = 'https://<实际API域名>/api'
$env:NO_COLOR = 'true'
$env:CARGO_BUILD_JOBS = '2'
trunk build --release --locked --public-url /patchwork/
```

生产构建未提供地址时，界面提示配置缺失，不会自动连接旧 Shuttle 服务。正式域名尚未配置，当前没有更改 GitHub Pages 或 Cloudflare 路由。GitHub CI 从仓库 Actions 变量 `PATCHWORK_API_BASE` 读取地址；未配置时仍可验证编译，但发布任务会在上传网页前检查地址，要求 HTTPS、路径为 `/api`、无查询参数和凭据。实际发布前需要设置该变量，并使后端 allowed_origins 包含前端的精确 Origin。

## WebSocket 与连接接管

`GET /api/ws` 单独检查精确 Origin，拒绝缺失/不允许的 Origin 和任何 URL 查询参数。HTTP CORS 仍保留 GET/POST/PUT/OPTIONS 和 Authorization/Content-Type。

升级后默认 5 秒内，第一条应用消息必须是 Protobuf Authenticate。认证前业务消息、文本、畸形消息均被拒绝，不进入大厅。只有带有效持久 sid 的令牌可以建立游戏连接；旧 JWT 必须先迁入。单帧上限 16 KiB，服务端发送队列和发送等待有界，不能无限等待慢客户端。

I/O 使用当前 actix-ws，`connection_session` 持有每个 socket 的认证和关闭生命周期；身份接管由单例 Actix SessionRegistry Actor 串行决定。没有引入第二套 HTTP 服务或旧版 WebSocket Actor 依赖。Lobby/Room 的业务 Actor 架构保持既定方向。

注册连接时，在数据库事务中重新验证会话并递增 users.connection_generation，然后替换内存中的有效连接，通知旧连接关闭。Stamp 包含 user_id、session_id、generation，全部来自服务端认证上下文。Dispatcher 和 Disconnect 都校验完整 Stamp：旧连接不能发起有效命令，也不能通过迟到 Disconnect 清除新连接。新进程继续递增数据库中的 generation，重启不复用旧值。

注册使用非阻塞 Actor future，在数据库中重新验证持久会话；同一用户最多一个注册在途，全局注册上限 128、Registry 邮箱容量 128。注册等待 PostgreSQL 响应时不会阻塞其他连接的派发和快照查询，此边界已有暂停响应测试。负载与容量实测仍属于 T53。

浏览器每 15 秒发送应用 Ping，45 秒没有 Pong 主动恢复；服务端的 45 秒有效应用输入期限独立于出站流量和数据库操作。网络断开与令牌到期采用指数退避恢复原身份、房间及待确认请求；被接管、注销或策略关闭时保留身份并停止自动争抢。房间宽限、累计预算、重启恢复、Resume/SyncAck 和背压已接入，详见 [断线恢复](CONNECTION_RECOVERY.md)。

阶段 4 已将可撤销连接许可与持久 generation 带入 Lobby/Room 排队命令，取得数据库锁后再次验证。阶段 5 的 Resume/SyncAck 与 T38 的正式游戏动作沿用同样的授权边界，见 [权威对局](AUTHORITATIVE_GAMEPLAY.md)。匹配仍未实现。

## 验证与范围

复现：

```powershell
cargo test --locked -p backend -p util_lib -p game_core
cargo clippy --locked -p backend -p util_lib -p game_core --all-targets -- -D warnings
node --test tools/ci/browser_session.test.mjs
cargo build --locked -p backend
D:\Tools\Python\python.exe tools/ci/postgres_suite.py --bin-dir D:\Tools\PostgreSQL\18.6\pgsql\bin
```

数据库 runner 执行 18 项集成测试，并启动真实临时后端验证 HTTP/WS；WebSocket 探针使用 websockets 13.1。本机通过场景包含旧接口兼容、过期访问令牌刷新后 user_id 不变、旧 JWT 验签/迁入限制、数据库昵称优先、并发轮换唯一成功、创建重试等待锁后重新检查凭据、代理丢弃创建/刷新 COMMIT 确认后的恢复、旧连接消息/断线栅栏、重启代次递增、注销撤销、Origin 拒绝、未认证业务拒绝及首帧超时。

阶段 3 另有 14 项普通 Rust 回归和 8 项浏览器状态机 Node 测试通过；后者验证断网/非 401 保留身份、401 刷新、丢响应、跨标签页串行和本地存储失败。Clippy 和 Trunk release 打包通过，前端保留 4 条原有未使用警告。阶段 4 的数据库/双浏览器空对局验收见好友房文档；后续完整规则与权威事务的最新验收见开发 TODO 的 T38。

Windows 沙箱可能限制 pg_ctl 创建受限令牌；本次通过已授权的本机临时测试流程运行。所有测试集群位于项目 artifacts，使用随机测试库和凭据，完成后停止清理。未连接 192.168.5.9，未部署、提交或推送。
