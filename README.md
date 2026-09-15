# patchwork.github.io

## 房间对战与部署设计

- [房间、FIFO 匹配、对局同步和已知缺陷修复方案](docs/ROOM_BATTLE_PLAN.md)
- [192.168.5.9 的 D 盘部署与 Cloudflare 中转方案](docs/DEPLOYMENT_PLAN.md)
- [开发 TODO：按阶段、依赖和验收条件推进](docs/DEVELOPMENT_TODO.md)
- [游戏逻辑基线：左右图版、初始纽扣与时间图版标记](docs/GAME_RULES_BASELINE.md)
- [完整对局开发方案：随机供给、行动结算、旋转翻折交互与底部时间横条](docs/GAMEPLAY_IMPLEMENTATION_PLAN.md)
- [T34 规则数据冻结：33 块拼布、轨道、计分和版本兼容](docs/RULES_DATA.md)
- [T35 状态：双方图版、供给、中立指示物、行动阶段与快照](docs/GAME_STATE.md)
- [T36 几何：旋转翻折、安全落点、7×7 与恢复校验](docs/GEOMETRY.md)
- [T37 行动：前进、购买、收入、特殊拼布与终局](docs/ACTIONS.md)
- [T38 权威服务端：真实开局、动作事务、幂等与恢复](docs/AUTHORITATIVE_GAMEPLAY.md)

后端方案采用 Actix Web + Actix，数据库为独立 PostgreSQL，通过 SQLx 访问；Cloudflare 仅用于 HTTPS/WebSocket 中转。

工程与协议基础、PostgreSQL 数据层、身份会话、好友房间、断线恢复及完整对局规则已实现。T39 已接入 Bevy 联机图版、外围拼布、落点预览与 Yew 操作栏；T40 双浏览器完整对局与恢复验收已完成，FIFO 匹配暂缓。交互说明见 [前端对局说明](docs/FRONTEND_GAMEPLAY.md)。Windows PostgreSQL 和后端已部署到 `192.168.5.9:D:\deploy_patchwork`，运行与备份恢复操作见 [Windows 部署记录](docs/WINDOWS_DEPLOYMENT.md)。阶段 9 已安装 Cloudflare 连接器、准备中转启停脚本，并通过 SSH 链路的 100 连接/50 对局压测；账号登录、固定域名中转和 Pages 公网联调仍待完成，见 [Cloudflare 部署与验收](docs/CLOUDFLARE_DEPLOYMENT.md)。

## 本地开发

一键双浏览器测试：双击 [启动脚本](tools/start-local-test.cmd)，结束后双击 [关闭脚本](tools/stop-local-test.cmd)。启动会构建并打开两个页面；关闭会清理本次临时数据库。参数、依赖和日志说明见 [LOCAL_TEST.md](docs/LOCAL_TEST.md)。

- [工程结构、独立后端启动和 CI 检查](docs/FOUNDATION.md)
- [Protobuf 生成、兼容基线与消息边界](docs/PROTOCOL.md)
- [PostgreSQL 初始化、迁移、事务仓储与隔离测试](docs/DATABASE.md)
- [身份持久化、凭据轮换、浏览器缓存和连接接管](docs/IDENTITY_SESSIONS.md)
- [好友房间、Actor 队列、事务回执和双浏览器验收](docs/FRIEND_ROOMS.md)
- [心跳、断线预算、重启恢复、事件补发与背压](docs/CONNECTION_RECOVERY.md)
