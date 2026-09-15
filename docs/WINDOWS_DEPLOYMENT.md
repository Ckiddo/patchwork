# Windows PostgreSQL 与后端部署记录

更新：2026-09-13。目标主机 `192.168.5.9`，SSH 别名 `word-server-5-9`，主机名 `DESKTOP-NCGG1I7`。本文件记录阶段 8 的实际部署与操作入口；Cloudflare 和公网前端配置属于阶段 9。

阶段 9 后端修复已于同日部署，**当前 release 为 `20260913T141829Z-f3d45ca0-dirty-b2f9dc6d`**，server SHA-256 为 `03f14f9f3e2a71a128a936e780218891c5f4df768fbc2d84ea6d57d2229479f1`；migrate SHA-256 为 `8932b09e16bea7b6236ef9dbb59dfc3d5b2bb01a4102cc138d2d0fdcb79674ac`。数据库仍为 18.6 / 6 项迁移，无 schema 变更。当前 release ZIP SHA-256 为 `9949dbf0ddc9fbbcb8c082ae63e19855863f40785c4b557980850954bc18c722`。跨房间并发修复、100 连接实测、中转脚本与剩余公网项见 [Cloudflare 部署记录](CLOUDFLARE_DEPLOYMENT.md)。下方阶段 8 的旧 release/恢复数据保留为历史验收基线。

## 实际部署

| 项目 | 配置 |
|---|---|
| 项目根目录 | `D:\deploy_patchwork` |
| PostgreSQL | 18.6，Windows x64，`D:\Tools\PostgreSQL\18.6\pgsql\bin` |
| PGDATA | `D:\deploy_patchwork\data\postgresql` |
| 数据库 / schema | `patchwork` / `patchwork`，6 项迁移，版本与 SHA-384 精确匹配 |
| PostgreSQL 服务 | `PatchworkPostgres`，自动启动，身份 `NT SERVICE\PatchworkPostgres` |
| 后端计划任务 | `PatchworkBackend`，开机启动，身份 `DESKTOP-NCGG1I7\PatchworkBackend`，非管理员 |
| 监听 | 数据库 `127.0.0.1:15432`，API/WS `127.0.0.1:18120` |
| 认证与持久化 | SCRAM-SHA-256、UTF-8、校验和开启；UTC、fsync、full_page_writes、synchronous_commit 开启 |
| 资源起始配置 | 32 个数据库连接、shared_buffers 128 MB、应用池 8、Actix workers 2 |
| 本机备份 | `D:\deploy_patchwork\backups`，按用户要求直接备份到该主机 |
| 凭据与日志 | `secrets`、`logs`，均在项目根目录；操作命令不携带密码 |

安装包 SHA-256：`59f8ce701c63c2ed623c665a5e51b3ef6f2e37ccf837b68ffeed0742d0ae6abd`。复用了开发电脑已下载的 18.6 Windows 包，上传后再次校验；数据集群为本项目新建。预检时 D 盘约 1.62 TiB 可用，15432、18120、15433、18121 均空闲。

主机原有 `re-xianyu-postgresql` 使用 `D:\deploy\runtime\postgresql-data`，原有 MariaDB 使用其他项目目录。Patchwork 初始化、更新和清理脚本只管理自己的目录、服务、任务和端口。

Windows 自己的服务账户 profile/注册表仍由操作系统管理；项目程序、数据、配置、凭据、应用日志及应用 TEMP/TMP 使用上述 D 盘路径。

## 启动、停止和检查

以下命令在 **192.168.5.9 的管理员 PowerShell** 中执行。开发机可通过已有 SSH 别名执行同一脚本。

```powershell
# 一键启动数据库和后端，等待 readyz 真正就绪
powershell -NoProfile -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\Start-Deployment.ps1

# 一键优雅关闭后端；数据库保留运行，以便持续归档和定时备份
powershell -NoProfile -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\Stop-Deployment.ps1

# 如需关闭整个项目，加 WithDatabase，只停止 PatchworkPostgres
powershell -NoProfile -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\Stop-Deployment.ps1 -WithDatabase

Invoke-RestMethod http://127.0.0.1:18120/healthz
Invoke-RestMethod http://127.0.0.1:18120/readyz
Get-Service PatchworkPostgres
Get-ScheduledTask PatchworkBackend
Get-Content D:\deploy_patchwork\control\backend-exit.json
```

开发机调用示例：

```powershell
ssh word-server-5-9 powershell.exe -NoProfile -ExecutionPolicy Bypass -File D:/deploy_patchwork/tools/Start-Deployment.ps1
ssh word-server-5-9 powershell.exe -NoProfile -ExecutionPolicy Bypass -File D:/deploy_patchwork/tools/Stop-Deployment.ps1
```

后端启动器先核验 release 中所有文件 SHA-256，再等待 PostgreSQL；程序会验证数据库 schema 和房间恢复状态。启动依赖最多等待 60 秒。`Run-Backend.ps1` 对异常退出明确限制最多重试 3 次、间隔 60 秒，退避期间也响应停止信号；计划任务自身的 RestartCount 为 0，避免重试次数叠加。数据库服务配置 5/15/60 秒三次重启，之后停止重试，24 小时重置失败计数。

停止时写入受 ACL 保护的 `control\backend.stop`。后端 250 ms 检查一次，进入 draining、完成关闭并释放监听；停止脚本最多等待 40 秒，超时会报错，不误杀其他进程。手动正常停止返回 0，不触发失败重试；下次开机会按自启配置运行。关闭数据库期间备份任务不可成功，恢复运行后检查健康文件。

## 构建、发布和回滚

当前工作区包含此前未提交的完整功能开发，release 不能仅用 Git HEAD 表示内容。`build_release.py` 保存基准提交、dirty 标记、源文件 SHA-256 清单、两个 EXE 的哈希、6 项迁移 SHA-384，以及 PostgreSQL 主版本。

```powershell
# 开发电脑，仓库根目录
cargo build --locked --release -p backend --bins
python tools/deploy/build_release.py
Get-Content artifacts/release-package.json
```

将产生的 ZIP 上传到 `D:\deploy_patchwork\staging`。在远端调用：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\Publish-Release.ps1 -Zip D:\deploy_patchwork\staging\release.zip -Sha256 <release-package.json中的Sha256>
```

发布脚本检查 ZIP 哈希和路径、EXE/源清单/迁移文件哈希，禁止覆盖已有 release；已有版本先做逻辑备份，优雅停止后端，再用迁移身份执行迁移并核对 schema。指针通过同卷原子文件替换切换到新版本，`readyz` 成功后记录 `config\last-publish.json`。程序更新不替换 PGDATA。

```powershell
# 必须选择 releases 下已核验且与当前 schema 完全兼容的版本
powershell -NoProfile -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\Switch-Release.ps1 -Release <版本目录名>
```

回滚先检查旧版本和当前库的迁移版本、数量及 SHA-384。schema 不兼容会在停服/切换前拒绝；不能用旧 EXE 指针替换来撤销数据库迁移。首次发布没有历史线上旧程序；本阶段回滚演练只证明两个相同后端二进制、不同发布清单之间的切换，以及 schema 不兼容时拒绝切换。

## 备份与监控

| 任务 | 主机本地时间 / 行为 | 产物 |
|---|---|---|
| `PatchworkLogicalBackup` | 每天 03:00 | 自定义格式 `patchwork.dump`、SHA-256、状态清单 |
| `PatchworkBaseBackup` | 每周日 03:30 | 流式 WAL 的 plain 基础备份、`pg_verifybackup` 校验 |
| PostgreSQL archiver | 持续，`archive_timeout=60s` | `backups\wal`，已封存 WAL 的持久化副本 |
| `PatchworkBackupMonitor` | 每 5 分钟 | `logs\backup-health.json`、Windows Application 事件 |

备份任务使用 SYSTEM 的文件访问权限，数据库连接仍使用专门的 `patchwork_backup`，具有 REPLICATION、pg_read_all_data、pg_monitor。业务账号 `patchwork_app` 不能迁移、建库或建角色；对象由 NOLOGIN 的 `patchwork_owner` 持有，迁移账号单独登录。

每次备份保存角色定义（不导出角色口令）、配置、受保护的密钥副本、部署工具、release 清单与校验清单到 `backups\metadata`。metadata 仅 SYSTEM/Administrators 可读，不复制到仓库或聊天。它包含恢复原浏览器身份所需 JWT/数据库密钥，因此恢复时须一起保管。

```powershell
# 手动生成逻辑 + 基础备份
powershell -NoProfile -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\New-Backup.ps1 -Kind All

# 检查归档链与备份时效
powershell -NoProfile -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\Test-BackupHealth.ps1
Get-Content D:\deploy_patchwork\backups\last-success.json
Get-WinEvent -FilterHashtable @{LogName='Application';ProviderName='PatchworkBackup'} -MaxEvents 10
```

归档采用共享读取，兼容 PostgreSQL 在 Windows 上仍持有已封存 WAL 的写句柄；写入唯一临时文件，WriteThrough/Flush(true)，比对 SHA-256 后再移动为正式文件。相同名字且内容相同可安全重试，内容不同拒绝覆盖。失败返回非零，PostgreSQL 保留原 WAL 并重试。[PostgreSQL 连续归档要求](https://www.postgresql.org/docs/18/continuous-archiving.html)

本阶段真实归档失败曾触发 `PatchworkBackup` 事件 4101，修复后健康文件恢复 Ok。备份作业失败写 `last-failure.json` 和事件 4102。监控同时检查归档延迟、备份新鲜度、D 盘剩余空间和活动 WAL 增长；不会给第三方发送消息。

保留策略目前为**保留所有完整逻辑备份、基础备份和 WAL，不自动裁剪**。后续需要释放空间时，必须先验证保留的基础备份及其连续 WAL 链。按用户决定不设置离机复制；本机同 D 盘备份不覆盖整盘/整机损坏。

## 恢复演练与验证边界

`Test-Restore.ps1` 创建 `patchwork_test_deploy_<时间>` 和 `patchwork_test_restore_<时间>`，所有对局写测试都落在隔离库。逻辑恢复使用 `pg_restore --exit-on-error`；PITR 从已通过 `pg_verifybackup` 的基础备份恢复到 `restore-tests\<时间>\pitr-data`，手动启动临时 `PatchworkRestore` 服务，端口 15433，归档关闭，并等待目标时间点恢复和 promotion 完成。

后端的 Windows 全局互斥覆盖同一台主机，所以演练暂时优雅停止普通后端，用相同低权限身份运行 18121 临时验收后端。验收完毕删除临时任务/服务、关闭 15433/18121、清理两座明确记录的测试库、还原 HBA，并启动普通后端。保留恢复文件和非敏感结果作证据，不覆盖正式 PGDATA。

```powershell
# 远端管理员 PowerShell；先确认没有仍未处理的 acceptance-context.json
powershell -NoProfile -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\Test-Restore.ps1 -Operation Setup

# 开发电脑，仓库根目录；经 SSH 隧道执行双 WebSocket 对局和恢复检查
python tools/deploy/acceptance.py

# 失败时保留证据。定位并修复后，在远端清理本轮临时环境
powershell -NoProfile -ExecutionPolicy Bypass -File D:\deploy_patchwork\tools\Test-Restore.ps1 -Operation Cleanup
```

脚本 `Initialize-Host.ps1` 只用于首次空目录初始化：已有标记、服务、账户、非空 PGDATA 或占用端口会拒绝继续。不要为了重新运行初始化而删除数据目录。初始化中断需要依据具体失败步骤恢复，普通更新用 Publish/Switch 脚本。

关键 Windows 修复已纳入脚本：initdb 降权令牌所需的临时用户 ACL；专用账户的 SeBatchLogonRight；Start-Process 提前缓存进程句柄以保留真实退出码；启动器显式执行异常重试，解决本机调度任务未按预期重试异常子进程的问题；恢复服务脱离 OpenSSH 作业生命周期；到达只读一致状态后继续等待恢复提升。这些问题不能通过“命令退出成功”替代实际服务与业务验证。[Windows 账户权限 API](https://learn.microsoft.com/en-us/windows/win32/api/ntsecapi/nf-ntsecapi-lsaaddaccountrights)

本轮回归：72 项普通 Rust、43 项真实 PostgreSQL 集成、严格 Clippy、cargo fmt、Windows release 构建，以及包含无窗口文件停止的新真实进程冒烟测试通过。完整前端 WASM/真实 Chrome 对局证据沿用 T40，本阶段不涉及前端改动。

## 阶段 8 最终证据（历史基线）

最终 release：`20260913T114855Z-f3d45ca0-dirty-8319f31c`。基准 Git HEAD 为 `f3d45ca0ec1bece7a3996e99a460177eaa722963`；包含未提交工作区内容，未提交/推送。

| 校验对象 | SHA-256 |
|---|---|
| 发布 ZIP | `3aa0b508114081001b13181b5007b433fc389d963415c3e45c8fc6a15934aadc` |
| patchwork-server.exe | `465388b68b9948b6f5a37f09c0b4e535c902fdfa0a2d29077e4c3833d26c27fb` |
| patchwork-migrate.exe | `7e8cef0f88d173e62842ba17d775d0feb276f3e191b863388d05611a2a006993` |
| 源文件清单 | `8319f31c0058cc47512b6ae0071730027da2fde497b835ca8e1a77369853c218` |

实测结果：

- 低权限身份 8 项检查全部通过；业务密钥可读、日志可写，管理员/任务密码和 PGDATA 不可读，tools/config 不可写。
- 一键关闭/启动数据库和后端通过；确认只释放/恢复 15432、18120。自启定义已验证、任务实际运行通过；没有为测试而重启整台共享主机。
- 强制结束经路径核对的后端 PID 24360，启动器记录原退出码 -1，自动启动 PID 22284；恢复就绪耗时 **61.2007 秒**，未手动触发替代重启。原有 `re-xianyu-postgresql` 的 PID 和状态不变。
- 新版 → 首次发布版 → 新版实际切换通过；两个 release 使用相同后端二进制，部署脚本源清单不同。错误 ZIP SHA-256 和不兼容 schema 校验均拒绝；最终指针与就绪状态正确。
- 隔离库 `patchwork_test_deploy_20260913113918` 完整自然对局 `e63b6159-2e8b-4e9a-aae1-20df8830ca0d`，版本 61、结果 **58:55**，重启后保持一致。
- 逻辑 dump 恢复到独立库，快照、版本、回执、事件及结果逐项一致，耗时 **1.0971 秒**。dump SHA-256：`5c47da4d9969ce37f0a37c28db00936c7b85759e89cc86235ca7f37f0e2d25a0`。
- PITR 目标 **2026-09-13 11:41:24.062453 UTC**。未完成对局 `4d4e63e1-43bc-4161-9f82-85292b01fe52` 恢复到版本 1；两局合计 **60 条回执、62 条事件、1 条结果**，与切点前完全一致，之后的第 61 条回执/下一行动被排除。
- WAL 日志最后完成提交 **11:41:23.645405 UTC**，在 **11:41:24.299668 UTC** 的下一笔提交之前停止。距指定切点约 **0.417048 秒**，这是事务间隔；目标前已提交状态没有丢失。
- 从基础备份校验/复制开始，到恢复服务完成重放、提升及验收 API 就绪，耗时 **31.1403 秒**。计时前已等待所需 WAL 归档可用；这不是整机灾难、人工排查或大数据量下的 RTO 承诺。
- 恢复后原玩家会话重新认证/恢复成功，原行动重试命中原回执，下一步真实行动提交成功。临时库、临时任务和 `PatchworkRestore` 服务已清理；正式库未进行写测试。
- 两个备份计划任务均以 SYSTEM 账户实际触发成功，退出码 0：逻辑任务约 3.47 秒、基础备份任务约 23.89 秒。后者产物 `backups\base\20260913T115230943Z` 已通过 pg_verifybackup；最终备份健康 Ok、Issues 为空。
- 最终审计时间 **2026-09-13 19:52:56 北京时间**，API PID 18544、PostgreSQL PID 6296，仅 18120/15432 两个本项目端口监听 loopback，15433/18121 已关闭；正式库 users/games 均为 0，临时数据库数为 0。远端 20 个部署 PowerShell 脚本与最终发布源清单逐项一致。

本机同盘误操作场景的 RPO/RTO 目标有上述小规模演练证据；归档异常、未完成 WAL 段、数据量增长和整盘丢失仍有不同的实际恢复边界。按用户决定没有离机复制，不声明整盘损坏场景可恢复。

主要原始证据位于仓库忽略的 `artifacts`：`release-package.json`、`t43-identity-check.log`、`t45-deployment-lifecycle.log`、`t45-postgres-suite.log`、`t44-final-publish.log`、`t47-scheduled-backups.log`、`t48-acceptance.json`、`t48-pitr-server.log`。远端持久记录位于 `control\deployment-audit.json`、`control\release-switch-test.json`、`control\deployment-test.json`、`restore-tests\20260913113918` 及 `backups` 的清单。

最终审计还有本地 `artifacts/deployment-audit.json`、`deployment-backup-latest.json`、`t44-script-integrity.log`；远端操作文档在 `D:\deploy_patchwork\docs`。归档统计保留了修复前的 33 次失败历史计数，最后失败时间早于最后成功归档，未清空历史来掩盖问题。

前几次演练暴露了 Windows 时间格式、子进程管道、OpenSSH 作业回收和恢复提升等待问题；失败证据保留，以上数字只引用修复后完整通过的最后一轮。Cloudflare Tunnel、域名、GitHub Pages API 地址和公网双浏览器验收均未在本阶段执行。
