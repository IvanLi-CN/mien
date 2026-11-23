# Mien 错误码目录

本文固定控制面与运行时的稳定错误码。错误响应结构为：

```json
{
  "code": "PROXY_ENGINE_NOT_READY",
  "message": "The shared proxy engine is not ready.",
  "request_id": "01J...",
  "retryable": true,
  "details": {}
}
```

`code` 是稳定契约；`message` 是给人阅读的英文或本地化消息，不作为程序分支依据。`details` 只能包含字段级错误、资源标识和非敏感诊断；不得包含代理凭据、UUID、私钥、Cookie、Token、完整分享链接或目标 URL。

## 1. 认证与授权

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `AUTH_REQUIRED` | 401 | 否 | 缺少会话或 Bearer Token |
| `AUTH_INVALID` | 401 | 否 | 会话过期、Token 无效或已被撤销 |
| `AUTH_RATE_LIMITED` | 429 | 是 | 登录或认证尝试超过限流 |
| `AUTH_CSRF_INVALID` | 403 | 否 | Cookie 会话写操作缺少或使用了错误 CSRF Token |
| `FORBIDDEN_SCOPE` | 403 | 否 | Token scope 不允许该操作 |

## 2. 资源与状态

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `PROFILE_NOT_FOUND` | 404 | 否 | 档案不存在或已永久删除 |
| `PROXY_NOT_FOUND` | 404 | 否 | 代理配置不存在 |
| `RUNTIME_NOT_FOUND` | 404 | 否 | 档案没有受监督运行时 |
| `TOKEN_NOT_FOUND` | 404 | 否 | API Token 不存在 |
| `PROFILE_ARCHIVED` | 409 | 否 | 档案已归档，不能执行该操作 |
| `RUNTIME_CONFLICT` | 409 | 否 | 同一档案已有其他启动/停止事务 |
| `IDEMPOTENCY_CONFLICT` | 409 | 否 | 相同幂等键携带了不同请求 |
| `RESOURCE_IN_USE` | 409 | 否 | 代理或引擎仍被运行档案引用 |

## 3. 输入与能力

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `VALIDATION_FAILED` | 422 | 否 | 通用请求字段错误；`details.fields` 给出字段级原因 |
| `FINGERPRINT_INVALID` | 422 | 否 | 指纹规格不一致或超出内核能力清单 |
| `PROXY_CONFIG_INVALID` | 422 | 否 | 代理类型、传输、TLS 或凭据字段无效 |
| `SHARE_LINK_INVALID` | 422 | 否 | 分享链接格式、协议或参数不受支持 |
| `RUNTIME_MODE_UNSUPPORTED` | 422 | 否 | 当前内核或宿主不支持所选运行模式 |
| `KERNEL_UNSUPPORTED` | 422 | 否 | 请求的字段或组合不在内核能力清单中 |
| `UNSUPPORTED_PROTOCOL` | 422 | 否 | 当前引擎能力清单不支持该协议或传输 |
| `TUIC_NOT_SUPPORTED` | 422 | 否 | v1 明确不支持 TUIC |

## 4. 配额与资源

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `QUOTA_EXCEEDED` | 429 | 是 | 并发运行档案数达到上限 |
| `INSUFFICIENT_RESOURCES` | 503 | 是 | 内存、磁盘或文件描述符不足 |
| `KERNEL_NOT_FOUND` | 500 | 否 | 已安装内核缺失 |
| `KERNEL_CHECKSUM_MISMATCH` | 500 | 否 | 内核文件校验失败 |
| `PROXY_ENGINE_CHECKSUM_MISMATCH` | 500 | 否 | Xray 文件校验失败 |

## 5. 代理引擎

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `PROXY_ENGINE_NOT_READY` | 503 | 是 | 共享引擎尚未启动或 API 尚未就绪 |
| `PROXY_ENGINE_API_UNAVAILABLE` | 503 | 是 | Xray 管理 API 连接失败 |
| `PROXY_ENGINE_START_FAILED` | 500 | 否 | 共享引擎无法启动 |
| `PROXY_ENGINE_CRASHED` | 503 | 是 | 共享引擎异常退出，正在重启 |
| `PROXY_INBOUND_CONFLICT` | 409 | 是 | 运行档案入站端口或标签已被占用 |
| `PROXY_OUTBOUND_REFCOUNT_MISMATCH` | 500 | 否 | 共享出站引用计数对账失败 |
| `PROXY_CONNECTION_FAILED` | 502 | 是 | 节点不可达、认证失败、TLS/REALITY 失败或超时 |
| `PROXY_DNS_FAILED` | 502 | 是 | 目标域名或节点解析失败 |
| `PROXY_NOT_READY` | 503 | 是 | 代理检查未完成或未通过 |

## 6. 运行时

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `RUNTIME_SPAWN_FAILED` | 500 | 否 | 内核进程无法创建 |
| `RUNTIME_CDP_TIMEOUT` | 500 | 是 | CDP 在超时内未就绪 |
| `SANDBOX_UNAVAILABLE` | 500 | 否 | Docker 中无法确认 Chromium 沙箱 |
| `DISPLAY_UNAVAILABLE` | 503 | 是 | 有头模式所需 Xvfb/宿主显示不可用 |


## 7. 存储与内部

| 错误码 | HTTP | 可重试 | 含义 |
| --- | --- | --- | --- |
| `STORAGE_BUSY` | 503 | 是 | SQLite 忙或短时写锁冲突 |
| `STORAGE_MIGRATION_FAILED` | 500 | 否 | 数据库迁移失败且需要人工处理 |
| `STORAGE_INTEGRITY_FAILED` | 500 | 否 | 数据完整性检查失败 |
| `INTERNAL_UNEXPECTED` | 500 | 否 | 未分类错误；必须带 `request_id`，不得返回堆栈 |

## 8. 规则

- 新增错误码必须同时更新本文、OpenAPI、事件 schema 的相关枚举和测试。
- 错误码一旦发布，只能弃用，不能改变语义后复用。
- 代理凭据、Cookie、Token、UUID、私钥和完整分享链接永远不能进入 `message` 或 `details`。
- 可重试错误必须返回 `retryable=true`；客户端按指数退避重试，并设置上限。
- UI 必须能根据 `code` 区分配置、引擎、节点和目标站点问题。
