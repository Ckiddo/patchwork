# Patchwork 自有主机部署方案

更新：2026-09-13。Windows PostgreSQL 和后端已部署到 `192.168.5.9:D:\deploy_patchwork`；实际操作、发布版本和恢复演练证据见 [Windows 部署记录与操作手册](WINDOWS_DEPLOYMENT.md)。Cloudflare 路由与前端公网联调仍属于阶段 9。

阶段 9 已完成连接器安装、API 路径离线校验和 SSH 链路 100 连接/50 对局压测；当前命名 Tunnel、域名和公网 Pages 发布仍等待 Cloudflare 登录。当前状态与测量边界见 [Cloudflare 部署与验收](CLOUDFLARE_DEPLOYMENT.md)。

## 1. 目标与地址核对

用户指定在目标主机 D 盘建立 `deploy_` 前缀目录，本方案选 `D:\deploy_patchwork`。后端进程、PostgreSQL 数据集群、日志、备份和项目专属配置都放在该目录。数据库选型已按用户要求改为 PostgreSQL；当前无已部署旧库需要迁移。

用户已在本次会话确认目标为 **`192.168.5.9`**，原输入的 `192.158.5.9` 不作为部署目标。已通过本机现有 SSH 配置以 sshuser 登录并完成只读预检；详细结果见第 7 节。

## 2. 流量路径

```text
GitHub Pages 前端（保持现有静态托管）
    │ HTTPS API + WSS
    ▼
Cloudflare 上的专用 API 子域名（域名待选）
    │ 命名 Tunnel
    ▼
目标 Windows 主机上的 cloudflared
    │ HTTP / WebSocket → 127.0.0.1:18120（后端已部署；Tunnel 待接入）
    ▼
D:\deploy_patchwork\releases\<版本>\patchwork-server.exe
    │ SQLx PgPool → 127.0.0.1:15432（独立 PostgreSQL 18.6）
    ▼
PostgreSQL 独立服务 / 数据库 patchwork
    │ PGDATA
    ▼
D:\deploy_patchwork\data\postgresql\
```

使用 Cloudflare Tunnel 的出站连接发布私有源站，不需要公网可路由源站 IP。普通代理 DNS A 记录不能把 RFC1918 私网地址变成可访问源站。Tunnel 仅转发，身份验证、匹配、房间、规则判定、数据库全部在自有主机执行。[Cloudflare Tunnel 官方说明](https://developers.cloudflare.com/tunnel/)

本方案不需要 Workers、Durable Objects、D1 或 R2。Cloudflare 是中转基础设施；正式域名、账号权限和网络可达性需实际检查，不能据此宣称端到端服务已经可用。

正式使用固定域名和命名 Tunnel。API 路由指向 loopback，关闭该域名的 API 缓存，确保 WebSocket 升级可通过；不在公共路径上加会破坏浏览器 WS 握手的交互式 Access 登录。后端自行验证用户；管理端点仅 loopback/SSH。恢复能力考虑 Cloudflare 中断连接这一正常情况。

## 3. D 盘目录

```text
D:\deploy_patchwork\
  releases\<version>-<git-sha>\  程序、迁移、资源清单、SHA-256
  config\                      非秘密设置与 current-release.txt
  secrets\                     JWT/服务凭据；限制 ACL，排除源码/日志
  data\postgresql\             PostgreSQL PGDATA，包含 pg_wal
  config\postgresql\           postgresql.conf、pg_hba.conf 等
  logs\                        应用、数据库、启动器和 Tunnel 日志
  backups\logical\             pg_dump 自定义格式备份与清单
  backups\base\                PostgreSQL 基础备份与清单
  backups\wal\                 WAL 归档，按可恢复链保留
  tools\                       项目启动、备份、恢复、探测脚本
  staging\                     上传与解包，校验后才切换发布
  tmp\                         项目 TEMP/TMP
```

共享 `cloudflared` 二进制优先使用主机已有受维护安装；如果需要新装通用工具，放 `D:\Tools\cloudflared`。PostgreSQL 可复用二进制放 `D:\Tools\PostgreSQL\<major>\`，本项目独立服务的数据与配置仍归 `D:\deploy_patchwork`。项目 Tunnel 的配置、凭据与日志同样归本目录。运行时不得依赖 C 盘用户 profile 存放数据库、项目凭据或业务日志。

不把数据库放入 releases；升级不覆盖 data。现有部署目录和共享数据库不作为本项目数据目录。

## 4. 后端和数据选型

- 独立 Windows x64 release 可执行程序，日志和工作目录显式指定；本机编译后上传，不要求服务器安装 Rust。
- Actix Web + Actix 的原生进程保持与参考架构一致，数据库采用同机独立 PostgreSQL，Rust 使用 SQLx PgPool。
- 独立运行身份只获得当前项目所需权限。服务自启用专用 Windows 计划任务 `PatchworkBackend`；由项目 PowerShell 启动器读取版本指针、设置目录并启动程序，任务失败自动有限退避重启，禁止重复实例。
- 如需独立 Tunnel 连接器，用专用启动方式/任务名 `PatchworkTunnel`；若已有共享连接器则先核对归属，不直接替换或重启其他业务的 cloudflared 服务。
- PostgreSQL 使用专用 Windows 服务 `PatchworkPostgres` 与低权限服务身份；配置服务恢复和自动启动。后端启动等待数据库可连接并校验 schema；数据库故障时后端就绪检查失败，不能继续确认新动作成功。
- `/healthz` 只代表进程存活；`/readyz` 需数据库读写可用、恢复完成、LobbyManager 可响应。管理和数据库端口不经 Tunnel 发布。

### 4.1 PostgreSQL 初始化与访问

建议 PostgreSQL 18 的部署时最新稳定小版本，安装前确认 Windows 发行包来源和受支持状态，锁定实际版本与哈希。正式集群使用 UTF-8，初始化时启用数据校验和并核对结果，时区统一 UTC；PGDATA 与数据库日志均显式落在 D 盘。

- 专用数据库 `patchwork`、业务 schema `patchwork`，PGDATA 为 `D:\deploy_patchwork\data\postgresql`。
- 已配置 `listen_addresses='127.0.0.1'`、`port=15432`。`pg_hba.conf` 仅允许明确的本机业务/迁移/备份角色，认证采用 SCRAM-SHA-256，禁止 trust 和对外放行。数据库不经 Cloudflare Tunnel 暴露。[连接配置](https://www.postgresql.org/docs/18/runtime-config-connection.html)
- 分离 `patchwork_owner`（NOLOGIN 对象所有者）、迁移登录角色、`patchwork_app`（仅必要 DML/序列权限）、备份角色；业务角色不得是 superuser，也不能创建数据库/角色或执行 schema 迁移。备份恢复/复制所需权限另授予专用角色。
- 初始 max_connections 建议 32，应用池上限 8，其余预留给健康检查、迁移、备份和管理；shared_buffers、work_mem 等根据主机其他业务和压测结果配置，不能按整机内存独占分配。
- 凭据文件位于 secrets 下并限制 ACL；应用通过受保护配置读取连接信息，命令行和日志不输出带密码 URL。libpq 工具使用显式 PGPASSFILE，避免默认落到 C 盘 profile。
- PGDATA 的 ACL 允许数据库运行身份管理，后端运行身份不直接读写数据库文件。非空 PGDATA 不重新 initdb；脚本遇到版本或集群归属不匹配立即退出。
- 保留 fsync/full_page_writes/synchronous_commit；设置应用角色的锁/语句/空闲事务超时，监控 autovacuum、死元组、长事务、WAL 和磁盘。详见房间方案第 9 节。

### 4.2 开发与迁移

开发/CI 已具备独立 PostgreSQL 测试集群、角色初始化、迁移命令及 SQLx 并发/故障测试，复现方法见 [DATABASE.md](DATABASE.md)。不能对正式 patchwork 库运行写测试。发布脚本已经集成显式 `patchwork-migrate`，使用迁移账号执行；运行时只做版本/校验和检查。数据库主版本升级单独制定 pg_upgrade 或逻辑导出导入流程，不能随应用回滚直接替换 PostgreSQL 二进制打开不同主版本 PGDATA。

## 5. 备份和恢复

联调阶段与发布前使用 `pg_dump --format=custom` 导出 patchwork，`pg_restore` 恢复到独立新数据库；检查退出码和 stderr。pg_dump 可在并发使用时生成一致导出，但单库导出不包含全局角色，角色/权限定义及服务器配置需独立备份，敏感材料限制访问。[pg_dump 官方说明](https://www.postgresql.org/docs/current/app-pgdump.html)

已经建立 **pg_basebackup 基础备份 + 连续 WAL 归档**。逻辑 dump 不能作为 WAL 重放的基础备份。计划任务每天 03:00 逻辑备份、周日 03:30 基础备份、每 5 分钟检查归档失败/延迟与 pg_wal 增长；归档程序校验并持久落盘才返回成功，拒绝覆盖同名不同内容文件。按用户 2026-09-13 的决定，备份直接保存在 192.168.5.9 本机，不建立离机复制。[基础备份](https://www.postgresql.org/docs/18/app-pgbasebackup.html)、[PITR](https://www.postgresql.org/docs/18/continuous-archiving.html)

备份清单记录 PostgreSQL 主/小版本、应用提交、schema、时间、大小、SHA-256；物理备份另记录时间线/LSN 和所需 WAL 范围。逻辑备份工具使用与源站相同主版本的配套版本；物理恢复使用兼容主版本。不将活跃 PGDATA 普通文件复制当作可恢复备份。

当前保留全部完整逻辑备份、已验证基础备份和连续 WAL，暂不自动裁剪。后续若压缩保留范围，必须先确认最早保留基础备份依赖的完整 WAL 链，不能仅按文件日期删除。D 盘低于 20 GiB、活动 pg_wal 超过 2 GiB、归档就绪文件滞留超过 2 分钟、逻辑备份超过 26 小时或基础备份超过 8 天未更新会在本机健康文件和 Windows 事件日志报警。

同 D 盘备份可支持应用误操作后的数据恢复；整盘或整机损坏不在此备份覆盖范围内。离机目的地不再作为当前阶段待确认事项，Cloudflare 仅中转请求。

逻辑恢复演练使用独立测试数据库，验证迁移版本、约束、用户数量、对局版本、结果唯一性和应用读写。物理/PITR 演练使用独立 PGDATA、服务名和端口，选择一条已知动作前后的恢复时间点验证结果。正式恢复前停止本项目写入并留存当前集群；不覆盖活跃 PGDATA，不因应用回滚恢复整台主机其他业务。

同机逻辑误操作场景的目标为 RPO 不超过 5 分钟、RTO 不超过 30 分钟；实际演练测量及起止边界见部署记录。`archive_timeout=60s` 是归档切换配置，不等于故障时一定满足 RPO。当前不声明整机或整盘损坏场景的 RPO/RTO 保证。

## 6. 发布步骤与验收证据

1. 复核目标 IP、D 盘路径、应用 18120 / 数据库 15432 候选端口、PostgreSQL 现有安装/服务/数据集群、登录权限、cloudflared 和可用域名。
2. 完成房间方案中的单元/并发/恢复测试，Windows release 构建；生成提交、依赖锁、schema、SHA-256 清单。
3. 上传至 staging 并校验哈希；安装/定位 PostgreSQL，首次部署初始化独立空 PGDATA、角色和数据库，创建服务、配置与 ACL。升级已有集群跳过 initdb。秘密通过主机本地文件/管理界面配置，不进入仓库、命令输出或聊天。
4. 先备份；排空或保存活动对局；仅停止本项目进程，执行兼容迁移，切换 current-release 指针。
5. 启动候选版本，核对实际进程路径、PID/监听端口、healthz/readyz、数据库位置、任务自启与重启恢复。
6. 本机回环 API/WS 验收通过后，再配置 Cloudflare 子域名和 Tunnel；凭据不放前端或启动参数日志。
7. 两个浏览器经公网 HTTPS/WSS 测试登录、匹配、开局、动作、重连、结算。通过后再更新前端正式 API 地址并发布 GitHub Pages。
8. 记录发布版本和真实测量结果；失败时停止新版本，按 schema 兼容性选择回退程序或恢复备份，再核对旧版本健康。

迁移采用向后兼容的扩展变更，破坏性字段删除延后。首次部署尚无旧 Patchwork 后端时，回滚意味着停用本项目新路由/进程并保留数据，不操作其他服务。

## 7. 远端预检结果

下表保留 2026-09-12 的首次只读检查记录。2026-09-13 已补查 PostgreSQL、15432/18120、D 盘和运行权限并执行部署；下表的“尚未/候选”是历史状态，当前状态以 [Windows 部署记录](WINDOWS_DEPLOYMENT.md) 为准。

| 检查项 | 实测结果 | 对部署的含义 |
|---|---|---|
| 目标 | `192.168.5.9`，主机 `DESKTOP-NCGG1I7` | 已核对，不是原先误写的地址 |
| 系统 | Windows 11，64 位，8 个逻辑处理器 | 可选择 Windows x64 原生程序 |
| 内存 | 检查时空闲约 7,637 MiB | 有初期测试空间；不能作为容量压测结论 |
| D 盘 | NTFS，总约 1,863 GiB，可用约 1,664.8 GiB | 容量可用于独立 PostgreSQL 集群；发布前复核 |
| 运行账号 | sshuser，当前令牌管理员检查为 true | 具备部署准备条件；实际专用运行身份和目录 ACL 在发布时设置 |
| 目标目录 | `D:\deploy_patchwork` 不存在 | 可作为独立新项目目录；本次尚未创建或试写 |
| 18080 / 18081 | 均由 `D:\deploy\runtime\python\python.exe` 占用 | 不占用、不停止现有进程 |
| 18120 | 检查时无 TCP 监听 | 选 `127.0.0.1:18120`，启动前再检查，端口未预留 |
| 已有数据库 | MariaDB 监听 `127.0.0.1:3306`，程序位于 `E:\deploy_word_server\mariadb\bin` | 已有业务运行中；Patchwork 使用独立 PostgreSQL，不改其数据库或配置 |
| PostgreSQL 安装/服务 | 上轮预检未专门检查 PostgreSQL | 当前不能断言已经安装或尚未安装，部署前补查 |
| 数据库候选 15432 | 上轮预检未检查该端口 | 仅为规划值，不能当作已验证空闲 |
| cloudflared | 未发现同名运行进程/服务或 PATH 命令；常见 D/C 安装目录未发现 | 尚未证明主机任何位置都没有安装；正式部署先定位，否则安装受维护版本 |
| Rust 工具链 | 当前 SSH PATH 未发现 cargo/rustc | 采用开发机 Windows release 构建，上传程序 |
| Tunnel 出站网络 | `region1.v2.argotunnel.com:7844`、`region2.v2.argotunnel.com:7844` TCP 均可连通 | 支持后续尝试 HTTP/2 Tunnel；没有验证 UDP/QUIC、TLS 握手或账号认证 |
| Cloudflare 正式域名 | 未检查账号、未选域名、未创建 Tunnel | 公网访问仍是待实施步骤 |

2026-09-13 的补查发现已有 `re-xianyu-postgresql` 服务，使用 `D:\deploy\runtime\postgresql-data`，部署全程未修改或重启它。Patchwork 使用独立 PostgreSQL 18.6 安装、服务、端口、PGDATA 和凭据。Cloudflare 域名与 Tunnel 仍未发布。

首轮 Tunnel 可明确选择 HTTP/2，以使用已经探测过的 TCP 7844；是否使用 QUIC 应另行测量。官方说明该端口 TCP 对应 HTTP/2、UDP 对应 QUIC：[Tunnel 防火墙要求](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/tunnel-with-firewall/)。

## 8. 与实施方案的关联

功能和回归标准见 [ROOM_BATTLE_PLAN.md](ROOM_BATTLE_PLAN.md)，任务进度见 [DEVELOPMENT_TODO.md](DEVELOPMENT_TODO.md)，实际发布、回滚与恢复证据见 [WINDOWS_DEPLOYMENT.md](WINDOWS_DEPLOYMENT.md)。
