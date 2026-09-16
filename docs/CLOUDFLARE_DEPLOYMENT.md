# Cloudflare 中转与公网验收

本文件记录阶段 9。主机仍为 `192.168.5.9`，后端及数据库仍在 `D:\deploy_patchwork`。Cloudflare 仅负责 HTTPS/WSS 中转，不保存用户或对局数据。FIFO 匹配继续暂缓。

## 当前状态（2026-09-13）

- GitHub Pages 已存在：`https://ckiddo.github.io/patchwork/`，发布源为 `gh-pages` 分支根目录。
- 当前 Git 凭据具备该仓库管理和推送权限；仓库变量 `PATCHWORK_API_BASE` 尚未配置。
- 已在 5.9 安装官方 `cloudflared 2026.9.1`，位置 `D:\Tools\cloudflared\2026.9.1\cloudflared.exe`。官方发布资产及远端文件 SHA-256：`2837888cc0f5d58f15b6dc478376de90b4d3ba5241c7947455d1e0a0df429712`。
- 已准备命名 Tunnel 配置、自启、启停及公网探测脚本；官方程序的 ingress 离线校验通过。
- 5.9 到 `region1.v2.argotunnel.com:7844` 和 `region2.v2.argotunnel.com:7844` 的 TCP 出站检查均通过。该检查不替代命名 Tunnel 的认证连接，也未单独验证 QUIC/UDP。
- Cloudflare 账号已完成授权；账号内命名 Tunnel `patchwork-prod` 已存在（UUID `4784d7e1-2968-4b9e-b098-92804b85b3b8`），JSON 凭据已安全写入远端 `D:\deploy_patchwork\secrets\cloudflared-tunnel.json`，仅 SYSTEM/Administrators 可读。实际 API 域名、DNS 记录、连接器配置和公网发布仍待完成。
- 2026-09-14 本地发布门禁已补强：Pages/本地 Trunk 构建显式指定 `assets`，发布 workflow 与公网探测共用 API 地址校验；公网域名、Tunnel 凭据和双浏览器公网验收仍未执行。

**离线规则校验和 SSH 转发测试不等于公网验收。T49–T52 必须等固定 HTTPS/WSS 地址可访问之后逐项完成。**

## 路由和权限

```text
GitHub Pages 前端
    → https://选定的API子域名/api/* 或 wss://选定的API子域名/api/ws
    → Cloudflare 命名 Tunnel
    → 192.168.5.9 cloudflared
    → http://127.0.0.1:18120/api/*
    → PostgreSQL 127.0.0.1:15432
```

使用本地管理的命名 Tunnel，将单隧道凭据文件放在远端受保护目录。账号授权证书 `cert.pem` 留在管理端，不交给运行账号。

- 第一条 ingress 必须同时匹配指定 hostname 和 `^/api(/.*)?$`，原路径完整转发。
- 最后一条 ingress 为 `http_status:404`。`/healthz`、`/readyz`、`/metrics`、`/apix`、其他 hostname 均不可转发到后端。
- 后端和 PostgreSQL 保持回环监听；cloudflared 指标只绑定 `127.0.0.1:20242`。
- 运行账号为独立普通本地用户 `PatchworkTunnel`，只赋予批处理登录权和 `edge/` 必需权限。任务名 `PatchworkTunnel`，开机启动，最多 3 次进程重试、间隔 60 秒。
- 连接器仅记录 fatal 级日志；控制脚本记录 PID、状态及退出码，不记录请求 URL、Header、JWT 或凭据文件内容。
- 后端保留 `Cache-Control: no-store`、固定 Origin `https://ckiddo.github.io`、WSS 首帧鉴权及查询字符串拒绝规则。

在 Cloudflare 为该 API hostname 设置缓存绕过规则，覆盖 `/api` 与 `/api/` 开头的路径；避免对 API/WSS 应用交互式登录挑战。这里只针对最终选定的 API hostname 配置，不修改其他站点规则。

参考：[命名 Tunnel 配置、路径匹配与校验](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/local-management/configuration-file/)、[连接器运行参数](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/run-parameters/)。

## 待完成的账号操作与接通步骤

1. 在 Chrome 的 Cloudflare 控制台确认账号内有效域名，选定一个未占用的 API 子域名。实际域名确定前不写入示例域名作为正式构建变量。
2. 已通过官方 cloudflared 登录授权并确认命名 Tunnel `patchwork-prod`（UUID `4784d7e1-2968-4b9e-b098-92804b85b3b8`）。域名确定后，将该子域名路由到此 Tunnel，并核对目标 zone 与已有 DNS 记录。
3. 已将该 Tunnel 的 JSON 凭据通过 SSH 放到 `D:\deploy_patchwork\secrets\cloudflared-tunnel.json`，仅 SYSTEM/Administrators 可读；凭据内容不写入聊天、命令参数或文档。
4. 在 5.9 执行以下命令，将占位值替换为已核对的 hostname 和 UUID：

```powershell
& D:\deploy_patchwork\tools\Configure-Tunnel.ps1 `
  -ApiHostname '已核对的API子域名' `
  -TunnelId '已核对的Tunnel-UUID' `
  -RuntimeVersion '2026.9.1'
& D:\deploy_patchwork\tools\Start-Tunnel.ps1
```

`Configure-Tunnel.ps1` 拒绝覆盖已有同名账号/任务，校验凭据归属及程序哈希，在安装自启任务前验证 ingress。文件位于 `D:\deploy_patchwork\edge`。配置完成后才运行启动脚本，启动必须等本地后端及 Cloudflare edge 就绪。

停止中转：

```powershell
& D:\deploy_patchwork\tools\Stop-Tunnel.ps1
```

中转启停和后端/数据库启停独立。停止中转会断开浏览器 WSS；数据库和已提交对局仍由后端保存。

5. 从开发电脑执行只读公网检查：

```powershell
& D:\Tools\Python\python.exe tools/deploy/probe_edge.py `
  --api-base 'https://已核对的API子域名/api'
```

探测检查 TLS、Cloudflare 响应标识、两次不缓存响应、CORS 预检、内部路径 404、真实 WSS 升级，以及错误 Origin/查询参数的拒绝。不会创建用户或房间。报告为 `artifacts/t50-public-edge.json`。

## 2026-09-16 身份访问错误修复

正式入口为 `https://ckiddo.github.io/patchwork/`，API 为 `https://api.ckiddo.fun/api`。Pages 页面及 JS/WASM 资源正常；正式 API 新建、验证、注销返回 200，未启用旧版身份迁入。原前端把网络、存储、服务异常和旧身份迁入拒绝统一显示为“身份服务暂不可用或会话已失效”，迁入被拒后没有正式恢复入口。

现按错误类型显示原因；仅旧版迁入或原会话续期明确返回 401 时，提供“使用新身份进入”。用户确认新身份不继承原房间/对局后，先把原完整会话记录备份到当前浏览器的 `patchwork_session_v1:<API>:backup:<UUID>`，再保存新建候选凭据并请求创建。原 `game_jwt_token` 保留。备份只是本地凭据副本，不保证旧身份仍可在服务端恢复；不复制到源码、日志或报告。

确认操作在同一 Web Lock 内重新验证旧身份，避免另一标签页已经恢复身份后又被替换。网络/403/429/5xx/存储错误不触发替换；备份失败不覆盖旧记录；新建响应丢失仍用原候选重试。

16 项 Node 会话回归通过。直接加载前端 `SessionClient` 对生产 HTTPS API 实测：模拟旧凭据迁入 401 → 显式确认 → 本地备份 → 新建 200 → 再次初始化验证 200，身份一致；该测试会话已注销。测试仅使用内存存储和独立测试身份，不读取或修改用户浏览器凭据。真实 Chrome 自动化因 `Codex auth token is unavailable` 无法连接，因此无法确认用户本机属于哪一错误分支，也不将本次接口检查计作 T52 双浏览器验收。

## 前端发布与回退

公网 API 探测通过后，设置 GitHub 仓库 Actions 变量 `PATCHWORK_API_BASE=https://实际API子域名/api`。`.github/workflows/deploy.yml` 和 `tools/deploy/probe_edge.py` 共用 `tools/deploy/validate_api_base.py`，拒绝空地址、非 HTTPS、错误路径、用户信息、查询参数、片段、非 443 端口和尾点主机名；`ci.yml` 使用相同变量构建 Pages 产物。客户端 WSS 从此地址转换，发布时不需另设 WebSocket 地址。

先核对待发布的完整源码和本地未提交改动，完成 CI，再更新 `gh-pages`。发布后以实际 Pages 页面为入口使用两个独立浏览器身份验证：创建身份、建房/入房、准备、开局、购买/旋转/翻折/放置、收入和特殊拼布、完整结算、断线与同账号恢复、后端重启后继续行动。应记录前端构建提交与资源哈希、后端 release 和二进制 SHA、PID/任务状态、数据库迁移及公网探测结果。

发布前保存 Pages 当前提交和仓库变量旧值；回退时恢复前端提交和相应 API 配置。后端按 [Windows 部署记录](WINDOWS_DEPLOYMENT.md) 使用 `Switch-Release.ps1`，只回退兼容 schema 的版本。数据库和备份始终留在 5.9；按用户决定不增加异机备份。

## 本轮压测与修复

脚本为 `tools/deploy/load_acceptance.py` 和 `Measure-Load.ps1`，只允许阶段 8 创建的 `patchwork_test_deploy_*` 验收库。50 个并发工作线程建立 100 条经过身份认证的 WebSocket 连接，通过屏障确认同时在线；中途强制终止测试后端，再以原身份恢复快照并继续全部对局。

链路是开发电脑 SSH 转发到 5.9 的测试后端 `18121`，不包含 Cloudflare、公网 TLS、Bevy 渲染或浏览器开销。动作延迟从发送行动到双方收到完全一致的已提交快照，包含网络、排队、事务与广播。每局行动后暂停 150 ms。

首轮压测发现两项问题并修复：

- 全局在线状态版本导致其他房间玩家退出时，本局事务被误判为权限失效。现改为检查本局双方的连接租约和同步版本；本局重连接管/重新同步仍能使旧请求失效。
- 高连接数验收后停止后端可能无限等待。现为 HTTP 排空及数据库连接池关闭设置总超时，并保留不含敏感内容的超时事件。

连接池采样使用 SQLx 0.8.6 的 `sqlx::pool::acquire` 数值耗时，只有验收配置启用 `database.log_pool_acquire=true`；正式配置默认关闭。逐条同步输出曾明显干扰 Windows 鉴权压测，因此最终实现仅在内存保存至多 100,000 个数值，正常退出时输出样本数、p95、最大值和丢弃数。验收拒绝出现样本溢出的统计。

该耗时包括获取连接及连接健康检查，不应表述为纯队列等待时间。强制崩溃会丢失内存采样，因此本轮池耗时窗口仅覆盖重启后的恢复与行动直到最终关闭。数据库锁等待为约 1 秒间隔采样，零采样不代表从未发生瞬时等待。

最终复测于 2026-09-13 14:19 UTC 完成，隔离库 `patchwork_test_deploy_20260913141858`：

| 指标 | 实测 |
|---|---:|
| 同时鉴权连接 / 对局 | 100 / 50 |
| 完整结束对局 / 提交行动 / 业务错误 | 50 / 2,950 / 0 |
| 行动 p50 / p95 / p99 / 最大 | 17.74 / 98.13 / 167.02 / 212.08 ms |
| 后端强制退出后重新启动至 ready | 3.85 秒（验收脚本主动重启） |
| ready 后重连并恢复快照 p95 / 最大 | 898.45 / 954.98 ms |
| 后端 CPU 占整机峰值 / p95 | 11.97% / 11.54%（8 逻辑处理器，28 次采样） |
| 后端工作集内存峰值 | 21.59 MiB |
| PostgreSQL 应用连接峰值 | 8 |
| 连接池获取 p95 / 最大 | 49.80 / 217.11 ms，7,934 次采样 |
| PostgreSQL Lock / LWLock 非零采样次数 | 0 / 4 |
| 数据库死锁增加数 | 0 |
| 测试后端停止流程耗时 | 14.61 秒，超时后限定关闭 HTTP，未人工结束进程 |

50 局在 PID `14332` 强制退出前均已有行动提交；重启为 PID `9420` 后全部恢复到相同游戏状态并完成结算。3.85 秒是主动重启测试，不能替代阶段 8 已测的约 61.2 秒启动器自动退避恢复时间。最终关闭出现 `http_drain_deadline`，随后执行限定的 HTTP 强制关闭并记录 `stopped`；本轮没有数据库关闭超时。该结果证明退出不再无限等待，不能宣称所有升级连接均完成了优雅排空。该短时压测不代表长期容量或公网用户延迟。

最终后端 release 为 `20260913T141829Z-f3d45ca0-dirty-b2f9dc6d`；server SHA-256 `03f14f9f3e2a71a128a936e780218891c5f4df768fbc2d84ea6d57d2229479f1`。发布前已执行本机逻辑备份。正式后端 PID `24060`、PostgreSQL 监听 PID `6296`，分别只监听 `127.0.0.1:18120`、`127.0.0.1:15432`；正式库用户/对局均为 0，临时库数量为 0，备份健康检查通过。其他项目 PostgreSQL PID `6044` 保持运行。

本轮验证包括 backend 普通测试、严格 Clippy、release 构建、43 项真实 PostgreSQL 集成测试、真实双 WebSocket 对局/接管/心跳/恢复回归，以及最终 release 的控制台与无窗口文件停止、日志脱敏和单实例冒烟。最终内存采样实现由上述远端 100 连接复测验证。本轮未重新构建或发布前端，未提交/推送 Git，也未将 SSH 验收标成公网双浏览器验收。

原始证据为被 Git 忽略的 `artifacts/t53-load.json`、`t53-samples.json`、`t53-postgres.log`、`t53-release-smoke.log`、`t54-audit.json`、`t49-runtime-install.log`、`t49-egress.log`、`t50-ingress.log`。远端保留本轮验收日志，测试库和验收任务已清理。
