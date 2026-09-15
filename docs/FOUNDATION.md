# 工程与协议基础：开发说明

对应 TODO T01～T05。此阶段建立工程边界和本地后端；后续数据层见 [DATABASE.md](DATABASE.md)，身份认证见 [IDENTITY_SESSIONS.md](IDENTITY_SESSIONS.md)，好友房见 [FRIEND_ROOMS.md](FRIEND_ROOMS.md)。T34–T38 已完成规则引擎和 [权威服务端事务](AUTHORITATIVE_GAMEPLAY.md)，联机棋盘交互由 T39 接入。

阶段 5 已补统一连接清理、恢复预算、持久化计时与补事件握手，见 [CONNECTION_RECOVERY.md](CONNECTION_RECOVERY.md)。Registry 数据库注册等待不再阻塞整个邮箱，活动 Room 串行执行每秒恢复观察。

## 模块边界

| 目录 | 当前职责 |
|---|---|
| `game_core` | 纯 Rust 座位/棋盘坐标、T34 规则定义、T35 玩家/供给/时间状态、T36 共享几何，以及 T37 原子行动、轨道收入、特殊拼布、奖励与自然终局；不依赖 Bevy、网络或数据库 |
| `util_lib/proto/patchwork/v1` | Protobuf 协议唯一源文件 |
| `util_lib/src/protocol` | 生成类型、版本/大小/消息结构检查和确定错误码 |
| `backend/src/api` | Actix HTTP API、兼容鉴权接口、CORS、健康检查 |
| `backend/src/game` | 单例 Lobby、唯一 Room、用户预留、pending 队列及持久化后广播 |
| `backend/src/persistence` | SQLx PgPool、迁移校验、事务仓储与恢复；详见 DATABASE.md |
| `backend/src/identity.rs`、`sessions.rs`、`api/ws.rs` | 持久身份认证、SessionRegistry 和 WebSocket 生命周期 |
| `backend/src/config.rs` | 非秘密 TOML 配置、密钥文件路径解析与安全错误 |
| `backend/src/instance.rs` | Windows 全局命名互斥对象，Linux 开发/CI 使用文件锁 |
| `tools/ci` | 真实进程冒烟测试、隔离 PostgreSQL 服务检查 |

所有 crate 共享根目录 Cargo.lock；删除原后端的无效独立锁文件。构建后端使用 `-p backend`，不对包含浏览器前端的整个 workspace 做原生平台构建。

## 本地启动（PowerShell）

日常双浏览器联调优先使用 [一键启动/关闭脚本](LOCAL_TEST.md)，自动构建、初始化隔离库并等待服务就绪。以下命令用于单独启动后端。

在项目根目录运行。配置文件只包含密钥路径；请在本地 secrets 目录准备至少 32 字节的随机签名密钥，不放进聊天、日志或 Git。

```powershell
cargo build --locked -p backend
Copy-Item backend/config.example.toml backend/config.local.toml
# 准备 backend/secrets/jwt.key 后启动；该目录和本地配置已被 Git 忽略。
.\target\debug\patchwork-server.exe --config backend/config.local.toml
```

配置必须显式提供，未知字段、非法工作线程数/超时、包含路径的 Origin 会被拒绝。`jwt_secret_file` 相对配置文件所在目录解析，不随启动器工作目录变化；生产可用绝对路径。

开发默认 `127.0.0.1:8000`，对应前端调试 API 地址；生产方案使用 `127.0.0.1:18120`，实际发布时单独配置。Origin 为 `https://ckiddo.github.io`，不能带 `/patchwork/`。

- `GET /healthz`：200，仅证明 HTTP 进程存活。
- `GET /readyz`：数据库健康、Lobby 可达且房间占用目录恢复后返回 200、rooms=ok；缺库/故障/恢复未完成返回 503。该就绪状态表示好友房可接受请求，不代表完整对战规则已经实现。
- 原三个 `/api/auth` 路径和原有成功 JSON 字段保留。昵称按 Unicode 字符数量限制为 20；身份资料已落库，创建响应增加 session_id/refresh_token。数据库未配置时认证返回 503；需先执行迁移再测试身份功能。
- Ctrl+C、Windows Ctrl+Break、Unix SIGTERM 触发有限时长的优雅退出；超时由 HTTP server 强制结束剩余连接。关闭过程中保留单实例锁。
- 日志只输出启动/退出事件和 HTTP 状态码，不记录 URI/query、请求头、正文、令牌或数据库连接串；第三方 verbose 日志不受 RUST_LOG 开启。

本地已有同名后端时第二实例拒绝启动，即使选择不同监听端口。没有新增远程部署或公开管理/停止接口。

## 检查命令

```powershell
cargo fmt -p backend -p util_lib -p game_core -- --check
cargo test --locked -p backend -p util_lib -p game_core
cargo clippy --locked -p backend -p util_lib -p game_core --all-targets -- -D warnings
cargo build --locked -p backend
D:\Tools\Python\python.exe tools/ci/smoke_backend.py
rustup target add wasm32-unknown-unknown
$env:NO_COLOR = 'true'
$env:CARGO_BUILD_JOBS = '2'
trunk build --release --locked --public-url /patchwork/
```

如果 Cargo 的 target 目录不在仓库根目录旁，需要显式指定 Bevy 内嵌资源目录：

```powershell
$env:BEVY_ASSET_PATH = (Resolve-Path .\assets).Path
trunk build --release --locked --public-url /patchwork/
```

冒烟测试创建项目 artifacts 下的临时配置与随机测试密钥，启动隐藏的本地测试后端，检查无数据库时认证返回 503、重复实例拒绝、日志不泄密、优雅退出与端口释放。数据库 runner 会传入专用测试库配置，额外验证持久身份和真实 WebSocket。结束后清除临时凭据，只保留脱敏记录。不连接远端，不使用正式用户或数据库。

## CI 与发布门槛

`.github/workflows/ci.yml` 在 PR 及被 main 发布流程调用时执行：

1. Windows/Linux：格式、协议/核心/后端测试、Clippy、实际二进制冒烟测试。
2. Ubuntu 一次性 PostgreSQL 18 服务：先检查基础服务，再创建随机命名测试库和独立角色，执行应用迁移两次及 SQLx 并发/故障集成测试。详见 DATABASE.md。
3. 浏览器：编译 GitHub Pages release WASM 并上传 dist artifact。

原 Pages 发布 job 等待以上全部成功后下载同一次工作流生成的 dist，才推送 gh-pages。未执行 git push；远端 Actions 是否实际通过必须以后续运行记录为准。

## 验证记录

阶段 1 的 2026-09-12 本机 Windows 记录：13 项 Rust 测试通过；foundation crates 格式检查、Clippy（warnings-as-errors）通过；真实后端冒烟测试通过；`trunk build --release --locked --public-url /patchwork/` 通过，生成 dist 下的 index.html、JavaScript 与 WASM。前端保留 4 条现有未使用代码/字段警告；本次验收覆盖构建，不代表浏览器完整对局已验证。阶段 2 的新增验证见 DATABASE.md。

本机复现记录保留于被 Git 忽略的 `artifacts/backend-smoke.json` 和 `artifacts/frontend-build.log`。前端构建须将本机 `NO_COLOR=1` 覆盖为 `true`；首次高并发编译依赖失败后，限制 `CARGO_BUILD_JOBS=2` 的重试成功。Linux 与 PostgreSQL service job 需要 GitHub Actions 实际运行验证，不能把工作流文件已写好当成远端测试已通过。

完整网络协议说明见 [PROTOCOL.md](PROTOCOL.md)，后续工作见 [DEVELOPMENT_TODO.md](DEVELOPMENT_TODO.md)。
