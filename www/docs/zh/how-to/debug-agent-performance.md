---
title: 用 sigiro 调试并优化智能体
description: 记录一次编程智能体会话，找出它把时间和 token 花在了哪里，再把这些证据变成更精简、更快的工作流。
sidebar:
  order: 5
---

本指南使用智能体自身的 OpenTelemetry 来回答四个问题：

1. 这次会话把实际耗时花在了哪里？
2. 哪个模型、工具或操作消耗的 token 或调用次数最多？
3. 哪些失败、重试和重复读取没有做任何有用的事？
4. 提出的优化是否让一次可比的第二次运行变得更好？

不要根据一个慢 span 或模型的自我报告来做优化。请使用有界的会话时间窗口，对完整的操作汇总排序，并保留每个结论背后的 SQL。

## 1. 启动一个隔离的本地 sigiro

安装最新版本并启动服务器：

```bash
curl -fsSL https://sigiro.com/install | sh
export PATH="$HOME/.local/bin:$PATH"

export SIGIRO_DATA_DIR="$HOME/.local/share/sigiro-agent-debug"
unset SIGIRO_CATALOG_URL
unset SIGIRO_S3_BUCKET SIGIRO_S3_ENDPOINT SIGIRO_S3_REGION
unset SIGIRO_S3_KEY_ID SIGIRO_S3_SECRET
unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN

sigiro serve
```

未设置 `SIGIRO_CATALOG_URL` 时，sigiro 把遥测数据存放在本地 Parquet 文件中，并把它们的元数据保存在本地 DuckDB 目录里。它不使用 PostgreSQL 或 S3。显式指定数据目录可以让这次实验与其他本地遥测数据分开。

在另一个终端中等待它就绪：

```bash
sigiro healthcheck
curl http://127.0.0.1:9999/v1/services
```

然后按照[将 agent 自身的遥测数据发送到 sigiro](/docs/how-to/agent-telemetry) 配置 Claude Code、Codex、Hermes Agent 或 Pi。

## 2. 记录一次有用的测试会话

选择一个有明确完成检查的任务。开始前记录以下内容：

- 准确的提示词；
- 仓库版本和工作区状态；
- 智能体、模型和配置；
- UTC 开始和结束时间戳；
- 缓存和依赖是否已预热；
- 预期产物和验证命令。

不加干预地运行一次任务。一个同时改变提示词、模型、仓库状态和缓存预热程度的基准测试，无法说明它为什么变快。

运行结束后，确认导出器产生了数据：

```bash
curl http://127.0.0.1:9999/v1/services
```

服务名由导出器决定。请在这里查到它，而不要假设它等于智能体的产品名。

## 3. 查询有界的会话时间窗口

用覆盖整次运行的 UTC 字面时间戳调用 `/v1/query`。先汇总操作次数和错误 span，再查看对应的 trace ID。结果是原始证据，不是诊断或因果判定。在把返回的行当作完整结果之前，请检查 `x-sigiro-truncated` 响应头。

## 4. 对实际耗时排序

按 `span_name`（如有，也可按路由/地址属性）对 span 分组，然后查找以下模式：

- 一次模型调用占据了 p95 和总耗时的大部分；
- 许多顺序执行的工具 span 本可以独立运行；
- 设置、认证或依赖检查在一次会话中重复出现；
- 失败的工具在输入不变的情况下被重试；
- 同一个文件、URL 或状态端点被反复获取；
- 子智能体重复了父智能体已经做过的探索；
- 审批或等待用户的时间被算作了执行时间；
- 一个长的父 span 只是包含了子 span 的时间，不能再与之相加。

span 的耗时是包含式的。不要把父会话 span 和它的子模型、子工具 span 当成相互独立的成本相加。用父 span 衡量端到端延迟，再用子 span 解释它。

编写自定义查询时，请使用 UTC 字面范围，这样 Parquet 统计信息才能跳过文件：

```bash
curl -sS -D /tmp/sigiro-headers -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT span_name,
                 count(*) AS calls,
                 sum(duration) / 1000000.0 AS total_seconds,
                 max(duration) / 1000000.0 AS slowest_seconds
          FROM sigiro_spans
          WHERE service_name = '<service-name>'
            AND timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
          GROUP BY span_name
          ORDER BY total_seconds DESC"
```

检查 `x-sigiro-truncated` 响应头。如果它是 `true`，在声称结果完整之前，请缩小查询范围或进一步聚合。查询端点只接受一条只读 `SELECT`，不接受 CTE。

## 5. 把证据变成改动

采用能解决测得的主要成本的最小改动：

- **被串行化的独立工具：** 一起发起请求，或把独立的调查并发委派出去。
- **重复的探索：** 只收集一次前置信息，把结果传给子智能体，而不是让每个子智能体重新探索。
- **过大的工具结果：** 在源头过滤，请求更少的字段，使用更窄的文件范围，避免把整份日志喂回模型。
- **过多的模型上下文：** 删除重复的指令和过时的历史；只检索当前决策需要的文件和记录。
- **重复的失败：** 重试前先修好缺失的路径、凭据、依赖或无效参数。输入不变的重试不算恢复。
- **过多的模型调用：** 把相互依赖的小问题合并成一个有界的请求，同时保持独立工作并行。
- **成本集中在一个模型上：** 机械性的探索用更便宜的模型，把能力强的模型留给真正需要它的决策。
- **漫长的审批等待：** 在声称执行速度有提升之前，先把人工等待时间和计算时间分开。

不要只凭 token 数推断因果。token 多既可能来自有用的推理，也可能来自重复的上下文。请阅读相关的 span、错误、结束原因和工具调用顺序。

## 6. 验证优化

在相同的仓库状态和可比的缓存条件下，再次运行同一个提示词。比较等长的 UTC 时间窗口，并报告：

- 端到端会话耗时；
- 模型调用次数和 p95 延迟；
- 有上报时，输入、输出、缓存和推理 token；
- 工具调用次数和子 span 累计耗时；
- 失败和重试的操作次数；
- 请求的产物是否通过了同一条验证命令。

任务失败的更快运行不算优化。请保留比较所用的有界 SQL 和查询结果。

## 让分析智能体直接访问

sigiro 提供两个面向机器的接口：

- `GET http://127.0.0.1:9999/openapi.json`，用于 HTTP API；
- `sigiro mcp`，用于 stdio MCP 工具：`list_services` 和 `run_query`。

对于 Hermes Agent，把下面的内容添加到 `~/.hermes/config.yaml`，确认 Hermes 的环境中已安装 Python 的 `mcp` 包，然后重启 Hermes：

```yaml
mcp_servers:
  sigiro:
    command: sigiro
    args: [mcp]
```

其他支持 MCP 的智能体也可以用各自的 MCP 配置注册同一条 `sigiro mcp` 命令。如果智能体服务的 `PATH` 比你的交互式 shell 更窄，请使用 `sigiro` 的绝对路径。

## 给分析智能体的提示词

### 找出会话瓶颈

```text
Use Sigiro to analyze service <service-name> from <UTC start> to <UTC end>.
Use list_services, then run_query with literal UTC bounds to rank where the
session spent wall-clock time by operation and model. Distinguish inclusive
parent duration from child duration; do not double-count nested spans. Cite
trace IDs or SQL results for every conclusion. Check truncation before
claiming completeness. End with the three smallest changes most likely to reduce
end-to-end latency without weakening the task's verification.
```

### 找出无用功

```text
Audit service <service-name> from <UTC start> to <UTC end> for work that did not
advance the task: repeated file or URL reads, unchanged retries, failed setup or
authentication calls, duplicate parent/child discovery, oversized tool results,
and serial calls that were independent. Query bounded raw telemetry, then inspect
complete rows for suspicious operations. Report counts, duration,
and trace IDs. Separate evidence from inference.
```

### 分析模型和 token 成本

```text
Analyze GenAI telemetry for service <service-name> from <UTC start> to <UTC end>.
Break down calls, p95 latency, errors, finish reasons, and available input,
output, cache, and reasoning tokens by provider and model. Identify where token
volume is concentrated and correlate it with the surrounding operation or tool
sequence. Do not recommend a cheaper model unless the evidence shows which calls
are mechanical enough to move safely.
```

### 比较优化前后

```text
Compare service <service-name> in baseline window <UTC start/end> with optimized
window <UTC start/end>. Use equal query logic and literal TIMESTAMP bounds. Compare
end-to-end duration, model calls and p95, token categories, tool calls, failures,
and retries. Verify both runs completed the same acceptance check. Cite SQL and
rows for every delta, state missing telemetry plainly, and reject improvements
that only moved time into an unmeasured child process.
```

### 区分准备工作和交付工作

```text
Analyze this agent session as two phases: prerequisite/setup work and delivery
work. Derive the boundary from span names and timestamps rather than assuming it.
Report duration, failures, retries, and model/tool calls in each phase. Identify
which setup checks can be cached or performed once, and which are required for
correctness. Include trace IDs and runnable drill-down SQL.
```
