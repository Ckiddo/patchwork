# Patchwork Protobuf v1

源码：`util_lib/proto/patchwork/v1/protocol.proto`。生成 Rust 类型通过 `util_lib::protocol::v1` 共享，协议检查入口为 `decode_client`。`/api/ws` 已接入认证、Ping/Pong、好友房、Resume、SyncAck 和正式规则的 GameRequest；认证规则见 [身份与会话](IDENTITY_SESSIONS.md)，房间契约见 [好友房间](FRIEND_ROOMS.md)，恢复握手见 [断线恢复](CONNECTION_RECOVERY.md)，动作事务见 [权威对局](AUTHORITATIVE_GAMEPLAY.md)。匹配和旧空对局的游戏动作仍返回 NOT_IMPLEMENTED；未完成同步的 Game 请求先返回 SYNC_REQUIRED。

## 生成与兼容检查

`util_lib/build.rs` 在 Cargo 构建时使用锁定依赖中的 vendored protoc 和 prost-build，将 Rust 类型及 descriptor 写到 OUT_DIR。无需修改机器全局 protoc 或提交生成的 Rust 源码，WASM 构建仍使用主机侧 protoc。

```powershell
cargo build --locked -p util_lib
cargo test --locked -p util_lib
```

`util_lib/proto/v1.schema.txt` 是可审阅的 descriptor 基线，固定字段名、编号、类型、cardinality、oneof 和 enum 值。字段变化会使测试失败，须先审查兼容性再显式更新：

```powershell
cargo run --locked -p util_lib --example protocol_schema |
    Set-Content -Encoding utf8 util_lib/proto/v1.schema.txt
```

测试归一化 LF/CRLF，避免 Windows 换行导致假差异。字段删除须 reserved 原编号/名称，不能重用；破坏性修改用新 package 和协议版本。允许新增未知字段，未知顶层/嵌套命令拒绝处理。当前基线测试是审查门槛，不能通过盲目重生成来证明兼容性；同时保留 ping/error 的固定 wire bytes 用例。

## 信封与作用域

- `ClientEnvelope`：protocol_version=1、request_id、单一 payload。request_id 为 1～64 字节的 ASCII 字母、数字、`-`、`_`，客户端建议使用 UUID。
- `ServerEnvelope`：版本、可选 request_id、event_seq、响应/推送 payload。普通请求的回执关联原 ID；主动推送无 request_id；解码失败不回显不可信输入。
- 房间和游戏分别使用自己的版本；事件 `(game_id, event_seq)` 去重。uint64 在 Rust/protobuf 中保留，不转成 JavaScript Number。
- Authenticate 不含可信 user_id，服务端验证 access token 后决定操作者；业务消息也不接受任意操作者 ID。
- Lobby 包含建房、加入、离开、显式准备、开局、分页列表、状态查询与 SetRules；Match 包含入队、取消、状态查询和确认。
- Game 包含动作意图和 expected_version；Resume 带 game_id/last_seq/has_snapshot。SyncAck 确认 game_id、version、event_seq 和本连接的 sync_token。客户端没有提交完整快照或任意结果的命令。
- GameSnapshot 的 state_json 为按 rules_version 定义的核心状态 UTF-8 JSON 字节。正式规则按 GameSnapshot 的完整校验反序列化，并核对 game_id 与 phase；不能将任意 bytes 当作可信游戏状态。

## 解码边界

单个客户端二进制消息上限 16 KiB，先判断大小再解码；畸形 protobuf、版本不支持、非法请求 ID、缺失/未知命令都返回确定 ErrorCode，不使用 unwrap 解析外部输入。

decoder 只校验传输结构；身份、权限、房间阶段和容量由 handler/事务校验。阶段 4 仅新增 SetRules、房间短码/模式/密码标记/先手、成员昵称和错误码 15–19，未改动既有字段编号和类型；兼容基线已审阅更新。协议类型不自动实现 Debug，避免认证 token 或密码被日志输出；只有无业务内容的错误枚举允许 Debug。

错误响应只包含稳定 code 和 retryable，不回传解析库原始错误、原始消息或令牌。后续 handler 必须复用此边界，并按业务约定填充幂等回执和状态快照。

阶段 5 仅增量添加 ClientEnvelope.sync_ack=16、ServerEnvelope.resumed=18、ResumeRequest.has_snapshot=3、GameSnapshot.phase=6、ResumeState/GameEvent/SyncAck 和错误码 20/21。原字段编号与类型保持不变，descriptor 基线已更新。Resume 最多补 64 条连续事件，缺口或过大时发快照；服务端信封上限 64 KiB，客户端帧上限仍为 16 KiB。

T38 沿用 advance=10、buy_and_place=11、place_special_patch=12、resign=13，动作成功返回 Acknowledged；仅新增错误枚举 GAME_NOT_RUNNING=22、NOT_YOUR_TURN=23、INVALID_PLACEMENT=24、INSUFFICIENT_BUTTONS=25、WRONG_ACTION_PHASE=26。descriptor 基线已同步审阅，现有字段和枚举编号不变。

正式 GameEvent 的 JSON kind 为 game_transition_v1，包含有序动作事实和提交后的完整核心快照；一次事务只递增一个 seq/version。浏览器校验连续游标并安装状态后确认同步，不重复执行费用或奖励。购买的 patch_id 是规范十进制字符串，quarter_turns 必须为 0～3；BoardPosition 使用 sint32 编码，坐标范围为 0～8。
