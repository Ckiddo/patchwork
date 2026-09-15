# PostgreSQL 基础与数据访问

对应 T06–T12，2026-09-12，随 T38 更新。数据层已经实现；身份认证见 [身份与会话](IDENTITY_SESSIONS.md)，房间 Actor 与好友房事务见 [好友房间](FRIEND_ROOMS.md)，完整动作、快照/事件和唯一结果事务见 [权威对局](AUTHORITATIVE_GAMEPLAY.md)。数据库仓储不是可直接调用的公网游戏 API；正式规则拒绝通用 begin_game/write_game 的外部快照写入路径。

## 模块和数据模型

- `backend/src/persistence/config.rs`：显式 host/port/database/user/password_file，密码从本地文件读取，类型不派生 Debug。本阶段仅允许 `127.0.0.1`，符合数据库与后端同机的部署架构。
- `backend/src/persistence/mod.rs`：共享 SQLx PgPool、schema 校验、业务账号权限校验和数据库健康检查。
- `backend/src/persistence/transaction.rs`：事务提交、确认回滚后的有限重试、稳定错误分类、提交结果未知。
- `backend/src/persistence/repository.rs`：用户初始化和占用补行，建房/入房/入队/配对事务，开局快照存储，对局快照/事件/命令回执/结果原子写入，回执恢复与分页查询。
- `backend/src/bin/patchwork-migrate.rs`：显式迁移命令；普通服务器启动不执行 DDL。

迁移版本：

| 版本 | 表与约束 |
|---|---|
| 0001 | users、sessions、rooms、room_members、matchmaking_tickets、operation_receipts、player_occupancy；UUID 身份、双座位、房主成员外键、房间短码唯一、单用户成员唯一、活动票据部分唯一索引 |
| 0002 | games、game_events、command_receipts、game_results；活动对局唯一、事件序号主键、请求作用域唯一、一局唯一结果 |
| 0003 | 延迟触发器校验占用/成员/活动票据一致；撤销业务账号修改 SQLx 迁移记录的权限 |
| 0004（阶段 3） | 恢复凭据轮换、旧 JWT 迁入标记、持久连接代次 |
| 0005（阶段 4） | Waiting 房间按 UUID 分页索引 |
| 0006（阶段 5） | 等待座位离线时间、room_recovery 的 epoch/tick/累计预算/恢复状态、允许服务端生命周期事件的 NULL user_id |

所有业务表位于 `patchwork` schema。ID 为 uuid，时间为 timestamptz，状态/事件/响应为 jsonb，散列为 bytea，版本为非负 bigint。由于 PostgreSQL bigint 是有符号整数，协议 u64 在进入仓储前须检查 `<= i64::MAX`，递增还要留出一位；仓储的对局版本/序号已经检查溢出。

占用行仅允许 Idle/Queue/Room/Operation 对应关联列的合法组合。延迟检查允许同一事务暂时出现中间状态，但提交时不能出现“玩家既在队列又在房间”或“成员存在而占用为空”。多玩家事务按用户 UUID 升序锁占用行，再按票据 UUID 升序锁票据，最后锁房间/对局。配对读取 FIFO 前两张有效票据，不使用 SKIP LOCKED 跳过较早玩家。

Operation 占用预留字段已建立，但事务仓储不把尚未提交的操作写成持久 Operation 状态。阶段 4 使用 Lobby 用户预留、Room pending 队列和锁内回执恢复；持久占用仅随真实成员变更提交。身份轮换和旧 JWT 迁入详见身份与会话文档。

阶段 5 的 `persistence/recovery.rs` 实现重启初始化、健康观察计时、超时/无胜者结果及一致快照/事件读取；tick UUID 防止提交确认丢失后重复扣时。最新恢复策略和 36 项数据库回归见 [断线恢复](CONNECTION_RECOVERY.md)。部署需先用迁移账号执行 0006，普通后端仍只校验 schema。

## 独立数据库与角色初始化

`tools/db/bootstrap.sql` 用于一个全新、专用的开发/测试集群，创建：

- `patchwork_owner`：NOLOGIN 对象所有者。
- `patchwork_migrate`：可 SET ROLE 到 owner 的迁移登录账号。
- `patchwork_app`：仅 schema USAGE、业务表 DML 和序列 USAGE/SELECT，无建库/建角色/schema CREATE 权限，不能写迁移记录。

脚本遇到已有同名角色或数据库会失败，不覆盖已有部署。在复用一个已有集群时，管理员须先核对角色归属并单独安排授权，不能把初始化脚本当作升级脚本。

管理员使用显式 libpq 连接参数和本地凭据文件运行。密码通过仅当前子进程可见的 `PATCHWORK_MIGRATE_PASSWORD` / `PATCHWORK_APP_PASSWORD` 输入，不能写进命令参数或版本库：

```powershell
psql -X -v ON_ERROR_STOP=1 -v db_name=patchwork_dev -f tools/db/bootstrap.sql
```

角色密码应另存到项目 secrets 文件。按 `backend/config.example.toml` 创建运行时配置，启用 `[database]`，用户名为 `patchwork_app`。另建一个迁移配置，使用 `patchwork_migrate` 和独立密码文件；迁移连接可适当增加语句/锁超时，不能直接照搬短业务超时处理未来的大迁移。

```powershell
cargo build --locked -p backend
.\target\debug\patchwork-migrate.exe --config backend/secrets/migrate.toml
.\target\debug\patchwork-server.exe --config backend/config.local.toml
```

迁移命令内嵌 SQLx migrations，使用 SQLx 迁移锁、版本和校验和。重复执行会校验已执行版本，不重复建表；`backend/build.rs` 保证迁移文件变化会触发重新编译。应用启动要求数据库迁移版本集合与程序完全一致、成功标记和校验和匹配，并拒绝拥有 schema CREATE 等管理权限的运行账号。现阶段采用严格版本匹配；后续滚动发布需要单独定义可接受的 schema 版本范围。

SQL 全部参数化并显式限定业务 schema。使用 `query` / `query_scalar` 的运行时绑定，不使用需要在线数据库的 `query!` 宏，因此普通构建无需 DATABASE_URL 或 `.sqlx` 离线元数据；SQL 类型和实际约束由独立数据库集成测试验证。

## 连接池、健康与失败处理

默认最小连接 1、最大 8，获取连接超时 3 秒，锁等待 2 秒，语句 5 秒，空闲事务 10 秒。每个新连接设定 UTC、受控 search_path 和 synchronous_commit=on。池在 HTTP worker 工厂之外创建并共享，优雅退出后关闭。

`/healthz` 继续只表示 HTTP 存活。`/readyz` 的 database 随探测变为 ok/unavailable；探测校验 schema 并执行回滚的无数据修改 DML，能识别断连、池耗尽和只读数据库，总时限 1 秒。阶段 4 已接入占用目录恢复；数据库健康且 Lobby/房间目录可用时返回 200，否则 503。缺库时 database=not_configured，完整对战规则不属于该就绪判定。

事务完成前不会返回 `Committed<T>`。后续 handler 只能在拿到该类型后更新内存并广播；数据层本身没有 Actor 或推送回调。SQLx 的原始错误可能包含行值，只在事务内部分类，外部只得到稳定的 StoreError；SQL 语句日志关闭。[SQLx 事务生命周期](https://docs.rs/sqlx/0.8.6/sqlx/struct.Transaction.html)

- 语句执行遇到 40001/40P01：显式等待 rollback 成功后最多重试 3 次，采用抖动退避；事务闭包必须仅执行数据库工作，不能发网络通知。
- 唯一键/外键/CHECK 冲突：返回 Conflict，不重试。
- 锁/语句超时或连接获取失败：返回 Unavailable，不返回已提交结果。
- COMMIT 明确返回约束/事务回滚错误：返回稳定失败；COMMIT 连接错误或其他不能证明回滚的情况：返回 CommitUnknown，绝不自动重放。
- COMMIT 阶段的 40001/40P01 当前保守返回 Unavailable，不在已消费的事务对象上假装确认 rollback 并重试；语句阶段的同类错误有完整回滚重试测试。

`recover_operation` / `recover_game_write` 会锁定原操作使用的用户/对局行，等候未决事务结束，再按请求 ID 和 payload 哈希查回执。返回 Some 是已提交的原结果；None 是锁内确认未提交。恢复查询本身失败时，调用方必须继续冻结关联操作，不能将查询失败当作回执不存在。

同 ID 同 payload 返回原回执，先于旧版本检查；不同 payload 返回 RequestIdConflict。`GameWrite` 的哈希覆盖完整、确定的服务端参数；阶段 4 补充结算与房间 Finished/版本的同事务更新，固定先锁房间再锁游戏。`begin_game` 保留为数据层基础接口，公网好友房使用 `friends.rs` 的授权、连接代次、开局回执及初始快照事务。完整规则合法性仍属于阶段 7。

## 可复现验证

本机已准备 PostgreSQL 18.6 二进制 `D:\Tools\PostgreSQL\18.6\pgsql\bin`，来源为 [EDB 官方二进制下载页](https://www.enterprisedb.com/download-postgresql-binaries)。下载包 SHA-256 为 `59F8CE701C63C2ED623C665A5E51B3EF6F2E37CCF837B68FFEED0742D0AE6ABD`（本机计算，用于复现该文件，不是厂商签名校验）。未注册常驻数据库服务。

```powershell
cargo test --locked -p backend -p util_lib -p game_core
cargo clippy --locked -p backend -p util_lib -p game_core --all-targets -- -D warnings
cargo build --locked -p backend
D:\Tools\Python\python.exe tools/ci/smoke_backend.py
D:\Tools\Python\python.exe tools/ci/postgres_suite.py --bin-dir D:\Tools\PostgreSQL\18.6\pgsql\bin
```

普通 `cargo test` 明确忽略需要数据库的集成测试；测试 runner 才会显式运行它们。runner 新建项目 artifacts 下的临时 PGDATA，监听随机 loopback 端口，启用 SCRAM 和校验和，使用随机测试凭据和 `patchwork_test_<随机值>` 数据库，验证配置名称与实际数据库名称后才执行写测试。它调用同一 bootstrap.sql、连续两次真实迁移命令，最后停止自己创建的集群并删除临时目录，只保留脱敏的 `artifacts/postgres-suite.json`。

CI 在一次性 PostgreSQL 18 service 中运行同一初始化、迁移和 Rust 测试；外部模式只接受 CI 的固定 loopback/service 角色，不读取生产 DATABASE_URL。CI 服务随 job 销毁。Windows 版本测试 runner 已实测；Linux CI 的实际执行记录需推送后确认。

当前数据库测试覆盖：权限与类型约束、跨表占用一致性、并发占用/最后座位、入房与排队竞争、FIFO 配对、回执去重和冲突、对局写入原子回滚与唯一结算、池耗尽、只读数据库、schema 不匹配、锁/语句超时、真实死锁、40001 注入的重试上限、提交连接丢失、真实丢弃 COMMIT 成功确认后查询恢复，以及 HTTP 就绪状态随数据库故障变化。

本节记录数据层阶段的测试范围。后续完整对局验收见 [FULL_GAME_ACCEPTANCE.md](FULL_GAME_ACCEPTANCE.md)。数据库和后台程序现已部署到 192.168.5.9，实际角色、备份、恢复与操作入口见 [WINDOWS_DEPLOYMENT.md](WINDOWS_DEPLOYMENT.md)；Cloudflare 路由仍待阶段 9。
