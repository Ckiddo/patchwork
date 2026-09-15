# 完整双浏览器与恢复验收（T40）

日期：2026-09-13。范围：本地好友房、`patchwork_custom_v1`、Bevy 对局、PostgreSQL 权威状态和恢复。使用已有的 Rust、Chrome、Python 与 `D:\Tools\PostgreSQL\18.6\pgsql\bin`，先关闭旧测试进程，再使用现有启动器。未发布到 GitHub Pages、Cloudflare 或远端主机；FIFO 匹配仍暂缓。

## 结论与证据分层

完整自然对局、两个独立身份的状态一致性、特殊拼布待放恢复和终局提交后崩溃恢复均通过。极端棋盘另用通过 `game_core` 校验的样本验证；协议层的伪造请求、旧代次和同 ID 不同载荷使用真实 WebSocket / PostgreSQL 回归，不能描述成鼠标操作。

| 层次 | 做法 | 证据 |
|---|---|---|
| 真实页面 | Chrome 的 `127.0.0.1:8082` 与 `localhost:8082`，独立存储和身份；点击 Bevy 图版及页面按钮 | 下列对局、版本、分数和双端校验值 |
| 浏览器边界样本 | 后端停止期间装载注明 `source=t40_fixture` 的快照，再恢复后端，通过页面完成动作 | `artifacts/t40-fixture-*.json`、数据库审计 |
| 协议和事务回归 | 真实 WebSocket、真实 PostgreSQL，以及核心规则测试 | `artifacts/t40-final-postgres.log`、`artifacts/t40-final-rust-tests.log` |
| 提交故障 | 本地 PostgreSQL relay 收到服务端 `CommandComplete(COMMIT)` 后丢弃确认，或在确认送达后端之前结束后端进程 | `artifacts/t40-faults.jsonl` |

页面 `data-state-checksum` 和原生规则校验程序使用相同的 `GameSnapshot` JSON 序列化与 FNV-1a 64 位摘要，覆盖双方棋盘、拼布实例、供给、中立指示物、时间、余额、收入、特殊队列、奖励与结果。摘要只用于诊断比较，不能作为认证或密码学完整性保护。正常对局另比较服务端版本、事件序号及数据库记录数。

## 完整自然对局

房间：`SBTQRNHHGJ`；对局：`609b23ee-d980-4ec0-bf47-176fcb087dc7`。

- A 为玩家 9147、座位 0；B 为玩家 6660、座位 1。由 UI 建房、加入、准备、开局。
- 初始 33 块、双方 5 纽扣、时间 0；初始候选 `10 / 9 / 31`。两端初始摘要均为 `cb8197d6242c8645`。
- 实际购买普通拼布 `9 / 12 / 28 / 17 / 27`，包括中间候选、旋转翻折和收入拼布；其余行动通过前进按钮执行。最终 B 的图版收入为每次 +8。
- 第 19 格特殊拼布待放时停止后端。断线页面保留状态并禁用操作；B 在后端离线时刷新，看到身份服务不可用提示。恢复后端后点击“重新加载”，仍为原身份、原座位和原对局，没有重置身份。
- 待放前版本 12，恢复后版本 14；规则状态摘要均为 `57be7e46ad688e44`。增加的是恢复事件，没有重复领取、收入或购买。
- 五块特殊拼布均通过页面放置：B 领取 19、25、43、49；A 领取 31。后经过的玩家不会再获得同一块。
- 最后 B 在 53、A 在 52。对 A 的最后一次前进注入 COMMIT 后崩溃，后端自动重启，两个页面恢复到终局；之后刷新 A，结果仍相同。

| 最终检查 | 结果 |
|---|---|
| A / B 时间 | 53 / 53 |
| A / B 纽扣及分数 | 49 / 55，B 胜 |
| A / B 已占格数 | 7 / 27，空格不扣分 |
| 双端及数据库摘要 | `305be3e4a5dfab4b` |
| 版本 / 事件序号 / 事件行数 | 46 / 46 / 46 |
| 动作回执 | 44 |
| 结算行数 | **1** |
| 样本注入事件 | **0** |

该局没有直接修改棋盘、时间、纽扣或结果；整局的购买、前进、特殊拼布放置均来自浏览器 UI。

## 边界样本验收

`81294195-bfd3-4f93-9e25-b3a167bf8099`（房间 `RD6TG8FZK2`）顺序装载四个独立样本。每次都有单独的 fixture 事件；它们不表示从上一个棋盘自然发展而来。尤其 supply / end_draw 会重置棋盘、余额和奖励状态，历史奖励事件计数必须结合样本段解释。

| 样本段 | 浏览器动作与结果 | 数据库核验 |
|---|---|---|
| `bonus_normal` | A 的 47 格棋盘补上 `#10`，形成 7×7；余额 20→18，HUD 显示 +7 分 | A 占 49 格，奖励归 A，奖励事件 1 条，摘要 `17e6bca174e0895b` |
| `bonus_challenger` | 保留 A 的奖励；B 在 19 格待放 1×1，补齐自己的 7×7 | 双方均 49 格，奖励仍归 A，累计奖励事件仍为 1，摘要 `e55fe1c2bc3f58cc` |
| `supply_two` | 仅剩 `#10`、`#27`；A 把 `#10` 右转一次放到 `(0,1)`，供给剩 1 块 | 中立指示物从空槽 30 到槽 32，下一候选跳过空槽 0 到槽 1 的 `#27`；摘要 `e6b810c08aab09d2` |
| 无可放位置 | B 可预览 `#27`，点击已有拼布处提示无可用落点；右键取消 | 无额外回执，余额和供给不变 |
| `end_draw` | A 时间 51、余额 0、收入 2；B 时间 53、余额 4。前进按钮显示“2 格、+4 纽扣” | A 仅获得 2 步奖励和 52 格的一次 +2 收入；双方 53 格、4/4 平局，1 条结果，摘要 `cd135ab033a3d59b` |

该边界房最终版本 / 事件 / 序号均为 20，动作回执 4、fixture 事件 4、结果 1。其结算仅说明最后的 end_draw 样本段。

满图版另开房 `LF8DEJW2F8`，对局 `026555be-9c8f-42e9-96c7-2cc71caa5217`：

1. `full_pending` 样本中双方在 53 格，A 占 80 格，五块特殊拼布待放；双方余额均 5。页面仍要求放置特殊拼布，没有提前结算。
2. A 左键点击唯一空格 `(4,4)`；同时注入 COMMIT 后崩溃。
3. 重启后 A 占 81 格，19 号特殊拼布已放置，25 / 31 / 43 / 49 以 `board_full` 原因弃置，队列为空。
4. 特殊拼布触发首次 7×7，A 获得 7 分；最终 12 / 5，双端和数据库摘要均为 `caff0b304f180b1a`。
5. 数据库版本 / 事件 / 序号均为 4，fixture 1、动作回执 **1**、奖励事件 **1**、结果 **1**。

## 供给完全耗尽的验收口径

冻结数据的 33 块普通拼布共有 **166 格**，两块 9×9 图版共 **162 格**；当前规则要求购买后完整放置，不能丢弃普通拼布。因此合法完整对局不可能买完全部普通拼布，特殊拼布还会占用额外空间。

浏览器已验证合法、无重叠的 2 块→1 块边界；没有伪造 0 块的完整对局。零候选和所有起始位置的绕环耗尽由 `game_core/src/state/tests.rs` 的 `every_starting_slot_can_be_exhausted_without_duplicate_candidates` 在独立供给组件层验证。T40.1 中的“耗尽”据此明确为组件边界，不能要求实际完整对局达到不可达状态。

## 验收发现与修复

### 访问凭证到期被误判为接管

真实长局发现：会话注册表每秒清理到期连接，可能先于 WebSocket 自身的到期计时器触发 kick。此前统一返回 1008，前端把它视为另一页面接管并停止重连，长局可能因此恢复超时。

`backend/src/api/ws.rs` 现在在处理 registry kick 时区分凭证是否已到期：到期返回可重连的 1001，未到期的接管仍返回 1008。另记录不包含用户、令牌或载荷的关闭原因和状态码，便于诊断。

新增真实 WebSocket 回归：签发 8 个仅持续 2 秒的本地测试凭证，每次到期必须收到 1001；既有接管测试同时要求旧连接返回 1008。浏览器长时间验收的日志也记录了 registry 到期返回 1001 后恢复，页面继续操作。

### 首轮与测试通道限制

内嵌浏览器首轮对局 `d033dbd6-a945-4212-b6c7-71b18bec535f` 验证了不足余额、越界、右键取消、双击购买加 COMMIT 确认丢失、连续行动和同格切换：丢失确认后只有 1 次购买回执。该局随后因到期断线排查超过宽限时间，以 A 超时弃权结束，**不计入完整自然终局通过**。

修复后内嵌通道重载仍出现额外接管现象，未单独定位其内部原因。最终完整验收切换到本机已有 Chrome，使用同一后端和数据库，通过上述自然局及边界局。验收结论仅覆盖实际使用的桌面 Chrome；不据此宣称所有浏览器内核、移动端或远程网络已通过。

Chrome 日志检查所见警告来自既有浏览器扩展，没有发现本轮 Patchwork 页面运行错误。未修改扩展设置。

## T40 覆盖对应关系

| TODO | 页面证据 | 核心 / 协议补充 |
|---|---|---|
| T40 | 完整自然局 49/55、待放恢复、终局 COMMIT 后崩溃、终局刷新、唯一结果 | 自然局逐动作规则重放和真实双 WebSocket |
| T40.1 | 两端随机状态一致、初始 #10、中间候选、2→1、环尾与跳空槽 | 不可达的完整耗尽改为供给组件穷举 |
| T40.2 | 连续行动、同格换人、旋转翻折合法放置、越界 / 重叠 / 不足余额、对手回合按钮禁用 | 非本人请求、所有几何变换、版本冲突 |
| T40.3 | 23→28 同时经过特殊拼布与收入标记、先到先得、19 待放断线、普通及特殊 7×7 | 一次越过多个收入 / 特殊点、队列顺序与唯一领取 |
| T40.4 | 51→53 只结最后一次收入、4/4 平局、满图版待放后 12/5、提交后重启 | 终局自动处理幂等、唯一结算、事务回滚 |
| T40.5 | 双击 + COMMIT 确认丢失只购买一次；自然及特殊终局各一次 COMMIT 后杀进程 | 相同 ID 重放、相同 ID 不同载荷、旧版本 / 旧连接、回执与事件原子提交、数据库阻塞回滚 |

## 工具与复现

正常手动测试仍用 [本地一键启动与关闭](LOCAL_TEST.md)。故障控制为显式开启的本地验收模式，生产后端没有测试 HTTP 接口。

```powershell
# 先关闭已有测试进程；构建原生规则检查器
powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools/local/Stop-LocalTest.ps1
cargo build --locked -p game_core --example t40_oracle

# 启用验收模式；如修改了代码，去掉 -SkipBuild
powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools/local/Start-LocalTest.ps1 -SkipBuild -NoBrowser -Acceptance

# 使用 UI 建房并取得真实 game_id 后，只读核验
D:\Tools\Python\python.exe tools/ci/t40_control.py inspect --game <game_id>

# 下一次游戏事务：数据库已提交，但丢弃给后端的确认
D:\Tools\Python\python.exe tools/ci/t40_control.py drop

# 或在下一次游戏事务提交之后、确认到达之前强制重启后端
D:\Tools\Python\python.exe tools/ci/t40_control.py crash

# 待放队列恢复；resume 后等待页面自动同步
D:\Tools\Python\python.exe tools/ci/t40_control.py pause
D:\Tools\Python\python.exe tools/ci/t40_control.py resume

# 只有后端暂停后才可装载已校验样本（会替换本局规则状态）
D:\Tools\Python\python.exe tools/ci/t40_control.py pause
D:\Tools\Python\python.exe tools/ci/t40_control.py fixture --game <game_id> --kind bonus_normal
D:\Tools\Python\python.exe tools/ci/t40_control.py resume

# 8 次短期凭证真实 WebSocket 到期检查
D:\Tools\Python\python.exe tools/ci/t40_control.py expiry
```

`inspect` 对照规则校验、版本 / 事件 / 序号、回执和结果行数，将摘要写入 `artifacts/t40-database-audit.jsonl`。控制器只接受本次验收启动器拥有的 `artifacts/postgres-test-*`、loopback 和 `patchwork_test_*` 数据库，不接受外部数据库 URL 或凭据；凭据只在本地受保护文件和子进程环境中使用。停止预览后控制器失效。

稠密棋盘已经固化在 `game_core/tests/fixtures/t40_packing.json`，正常验收无需求解器。可选重新生成：

```powershell
D:\Tools\Python\python.exe -m pip install --target artifacts/t40-python ortools==9.15.6755
D:\Tools\Python\python.exe tools/ci/t40_pack.py
```

生成结果写 `artifacts/t40-packing.json`；当前固化样本包含 32 块普通拼布、158 格，仅余 #27，去掉其中 #10 即为两块余量。新生成结果必须经过规则检查后再替换固化文件。

## 回归与收尾记录

- 最终修复后的 72 项普通 Rust 测试、43 项独立 PostgreSQL 集成测试通过；10 项浏览器会话模块测试通过。最终进程回归退出码为 0，真实身份、8 次短期凭证到期、好友房、强制重启恢复、完整自然对局、45 秒心跳、单实例、日志脱敏、就绪与优雅退出全部通过，见 `artifacts/t40-final-*.log`。
- 前端 WASM 检查和 release 构建通过，日志 `artifacts/t40-wasm.log`、`artifacts/t40-trunk.log`；严格 Clippy 与格式检查记录在 `artifacts/t40-final-clippy.log`、`artifacts/t40-fmt.log`。
- 故障触发共 3 次：一次确认丢失、自然终局及特殊终局各一次 COMMIT 后崩溃。
- 浏览器预览已关闭；`artifacts/t40-stop.log` 确认 8000 / 8082 端口释放，临时数据库已删除。脱敏后端日志另存为 `artifacts/t40-browser-backend.log`，审计与编译产物保留。
- 本轮未提交、推送或部署。移动端、减少动画偏好、外网延迟及 Cloudflare 中转的验收另行进行。
