# 本地双浏览器测试：一键启动与关闭

更新：2026-09-13。用于本机开发和好友房联调，所有服务只监听 loopback。T38 已接通正式规则的服务端动作、事务和恢复，见 [权威对局说明](AUTHORITATIVE_GAMEPLAY.md)；页面默认创建 patchwork_custom_v1，使用现有 Bevy 场景进行正式联机对局，交互见 [前端对局说明](FRONTEND_GAMEPLAY.md)。

## 一键操作

在资源管理器中双击：

- [tools/start-local-test.cmd](../tools/start-local-test.cmd)：构建后端及前端、初始化隔离 PostgreSQL、启动 API 和静态页面，等待就绪后在默认浏览器打开两个地址。
- [tools/stop-local-test.cmd](../tools/stop-local-test.cmd)：通过本项目保存的进程记录发送停止信号，关闭页面服务和后端，停止并清理本次临时数据库。

| 用途 | 地址 |
|---|---|
| 玩家 A | http://127.0.0.1:8082/ |
| 玩家 B | http://localhost:8082/ |
| API | http://127.0.0.1:8000/api |
| 就绪检查 | http://127.0.0.1:8000/readyz |

两个页面地址具有独立的浏览器存储，可以作为两名玩家建房、加入、准备。后台日志显示 Ready 后再开始操作。关闭脚本不关闭浏览器标签页。

开始对局后直接使用 Bevy 画布：绿色框内选拼布，右侧自己的图版移动鼠标预览，旋转/翻面后左键点击合法落点即可放置，也可按 Enter 或点击操作栏确认；右键或 Esc 取消草稿。左侧为对手图版。可以点击前进，跨过特殊拼布时按提示先放置 1×1，同样支持左键点击放置。底部横条始终显示完整时间轨道。

上一次临时数据库已清理、浏览器仍保存旧身份时，错误页会出现“重置本地测试身份”。确认页面说明后点击即可建立新的测试身份；正在进行同一局的刷新与重连不要重置身份。该按钮仅由本地启动脚本的 PATCHWORK_LOCAL_TEST=true 构建开关启用，正式构建不包含可见入口。

构建一般需要 1–5 分钟，首次依赖编译可能更久；可以查看构建日志确认进度。就绪后重复启动会复用当前进程，不重复构建或创建第二个库。重复关闭会报告已停止。

**每次启动都是新的临时测试库。关闭会删除该次账号、房间和对局数据。** 这延续原有双浏览器临时测试方式，不是持久化开发数据库，也不使用远端数据库。需要同一局刷新/重连测试时，保持这组进程运行；正常关闭后不保留旧局。

## PowerShell 入口

仓库根目录执行：

```powershell
# 完整构建、启动，并打开两个浏览器页面
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\local\Start-LocalTest.ps1

# 复用现有产物；修改代码后应使用上面的完整入口
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\local\Start-LocalTest.ps1 -SkipBuild

# 自动化检查时启动服务但不打开标签页
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\local\Start-LocalTest.ps1 -SkipBuild -NoBrowser

# 关闭本项目创建的测试进程
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\local\Stop-LocalTest.ps1
```

ExecutionPolicy Bypass 只作用于这次 PowerShell 进程，不修改系统策略。后台 runner 使用隐藏窗口；只有用户双击的启动/关闭入口显示进度。脚本使用 PID、进程启动时间和程序路径确认 runner 身份，不通过端口号或进程名批量杀进程。

## 本机依赖与目录

默认复用已经安装的程序：

- PostgreSQL：D:\Tools\PostgreSQL\18.6\pgsql\bin。
- Python：D:\Tools\Python\python.exe。
- Cargo：E:\tools\Rust\cargo\bin；Trunk：C:\Users\81564\.cargo\bin。

Python 与 PostgreSQL 位置可通过 Start-LocalTest.ps1 的 -Python、-PostgresBin 参数覆盖；Cargo/Trunk 仍可由 PATH 定位。脚本只为本次构建设置环境变量，不修改全局 PATH。前端以本地 API 地址和根路径构建，不覆盖正式 GitHub Pages 的 dist。

| 位置 | 内容 |
|---|---|
| artifacts/browser-dist | 本地前端 release 产物 |
| target/debug | 本地后端和迁移程序 |
| artifacts/postgres-test-* | 每次随机命名的 PostgreSQL 数据目录、临时密钥和数据库配置，关闭后删除 |
| artifacts/local-test-build.log | Cargo / Trunk 构建日志 |
| artifacts/local-test-runner.log | 启动、就绪和临时集群进度 |
| artifacts/local-test-runner.err.log | runner 错误日志 |
| artifacts/local-test-session.json | runner PID、启动时间、程序路径和测试地址，不含密码 |
| artifacts/browser-preview.stop | 关闭信号 |

数据库角色、密码和端口由现有 [postgres_suite.py](../tools/ci/postgres_suite.py) 为本次测试生成；执行迁移两次以验证迁移可重复。原始凭据不进入脚本源码、文档或进程记录。数据库、后端和静态文件服务都属于本项目；不会管理其他 PostgreSQL 服务。

## 出错时

- 8000 或 8082 被占用：脚本会退出，不结束未知进程。先确认是不是另一套本项目测试环境。
- 构建失败：查看 local-test-build.log；Windows PowerShell 5 的普通编译 stderr 不作为失败，脚本检查真正退出码。
- 启动失败：查看 local-test-runner.err.log，以及该次临时目录中的 PostgreSQL 日志。不要重复初始化已有非空集群。
- 关闭超时：脚本报告仍未关闭，不按过期 PID 强杀。保留进程记录和日志定位原因。
- Codex 受限执行环境出现 PostgreSQL restricted token 错误：这是沙箱进程令牌限制，应使用正常本机进程权限运行同一脚本；用户在资源管理器中启动不经过该沙箱。

后台初始化收到关闭信号时不会抹掉该信号。编译阶段尚未启动服务；请等待启动结果后再使用关闭入口。

## 显式故障验收模式

正常试玩不需要故障代理。需要验证待放恢复或数据库提交确认丢失时，先关闭旧预览，再使用现有启动器的 `-Acceptance`：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\local\Stop-LocalTest.ps1
cargo build --locked -p game_core --example t40_oracle
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\local\Start-LocalTest.ps1 -SkipBuild -NoBrowser -Acceptance
```

该开关仅给本次临时 PostgreSQL 和后端增加本地故障控制，不增加生产服务接口。`tools/ci/t40_control.py` 支持只读核验、暂停/恢复后端、下一次游戏 COMMIT 确认丢失、提交后杀进程，以及明确标记的规则边界样本。完整命令及测试数据替换范围见 [T40 验收报告](FULL_GAME_ACCEPTANCE.md#工具与复现)。未开启验收模式时不要运行这些控制命令；重新运行启动器不会改变已经运行的预览模式。

`pause` 保留本次数据库和静态页面，`resume` 恢复同一个后端；这与 `Stop-LocalTest.ps1` 关闭并删除整个临时库不同。后端离线时整页刷新会显示身份服务不可用，恢复后点“重新加载”继续原身份，不要点重置身份。

## 与自动回归测试的区别

上述入口用于长时间手动操作两个页面，不自动代替 Rust/数据库测试。自动回归使用：

```powershell
cargo test --locked -p backend -p util_lib -p game_core
cargo build --locked -p backend
D:\Tools\Python\python.exe tools/ci/postgres_suite.py --bin-dir D:\Tools\PostgreSQL\18.6\pgsql\bin
```

运行完整自动回归前先关闭本地预览，以释放 Windows 后端 EXE 和单实例锁。完整 runner 会创建另外的隔离库，执行数据库测试和真实后端进程冒烟，结束后自动清理。

## 验证记录

2026-09-13：完整入口实际完成后端构建、Trunk release 构建、隔离库初始化与服务就绪；首次此次前端构建约 3 分 39 秒。已检查 127.0.0.1:8082 页面和 API readyz。重复启动复用同一 runner；关闭后页面/API 端口释放，runner 确认数据库删除；重复关闭成功。自动验证使用 -NoBrowser，默认打开两个标签页的分支未做浏览器 UI 自动化验收。

脚本会保留构建产物和脱敏运行日志。关闭成功需要 runner 退出、端口释放以及日志中的集群清理确认；不会仅凭页面不响应就宣称数据库已清理。
