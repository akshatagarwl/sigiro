---
title: sigiro 文档
description: sigiro 存储 OpenTelemetry 链路、日志、指标和性能剖析数据，并回答针对它们的 SQL 查询。以智能体为先设计，托管工作区还为人提供仪表盘。
sidebar:
  order: 0
---

sigiro 存储 OpenTelemetry 链路、日志、指标和性能剖析数据，并回答针对它们的 SQL 查询。它以智能体为先：
每个答案都是一条智能体或人都可以再次运行的查询。托管工作区还为人提供一个只读的
[仪表盘](https://sigiro.com/dashboard/)。

第一次使用，想把自己应用的数据发到 sigiro.com？请从[向托管版 sigiro 发送遥测数据](/docs/how-to/hosted-onboarding)
开始。想在自己的机器上运行？请从[快速开始](/docs/tutorials/quickstart)开始。

本文档分为四个部分。每个部分回答一类不同的问题，请直接前往与你当前问题相符的部分。

## 教程

一堂课。它不假设你了解 sigiro，所以请先读这一部分。按步骤操作，每一步都能看到结果。

- **[快速开始](/docs/tutorials/quickstart)** — 在自己的机器上运行服务器，向它发送遥测数据，
  并用 SQL 查询最近的失败。只需五分钟。

## 操作指南

完成某一项任务的步骤。当你清楚自己想要什么、需要具体步骤时，请读这一部分。

- **[使用智能体为代码添加检测](/docs/how-to/install-with-ai)** — 给智能体一条提示词。智能体会为你的服务接入
  OpenTelemetry，并把它指向 sigiro。
- **[使用 Agent Auth 为代理完成注册](/docs/how-to/agent-auth)** — 注册一个委托的机器身份，只批准它需要的
  capability，然后轮换或吊销它的密钥。
- **[向托管版 sigiro 发送遥测数据](/docs/how-to/hosted-onboarding)** — 在网页上或用 `sigiro signup` 注册，
  从你的应用向我们运营的主机发送 OTLP，并在仪表盘中查看。
- **[将 agent 自身的遥测数据发送到 sigiro](/docs/how-to/agent-telemetry)** — 让 Claude Code、Codex、
  Hermes Agent 或 Pi 把数据发到 sigiro。
- **[调试并优化智能体](/docs/how-to/debug-agent-performance)** — 找出一次会话把时间和 token 花在了哪里，
  去掉无用的工作，并用证据比较下一次运行。
- **[将 sigiro 接入 Executor](/docs/how-to/executor)** — 把 OpenAPI API 或文档 MCP 服务器导入智能体网关。
- **[向 sigiro 发送 Pyroscope 性能剖析数据](/docs/how-to/send-profiles)** — 配置一个经过验证的性能剖析 SDK，
  然后用 SQL 读回剖析数据行。

## 参考

关于这台机器的事实。在工作中需要确认名称、类型或限制时，请读这一部分。

- **[API 参考](/docs/reference)** — 服务器的 `/openapi.json` 当前描述的 HTTP 操作。不在该文档中的接收协议
  （包括兼容 Pyroscope 的路由）在操作指南中说明。
- **[CLI 参考](/docs/reference/cli)** — 七个子命令、每个参数，以及它们读取的每个环境变量。

读取 `/openapi.json` 的智能体不需要我们提供任何其他信息。

## 原理解释

sigiro 为什么这样工作。当你想了解原因、代价和替代方案时，请读这一部分。不需要终端。

- **[数据表，以及它们描述的是哪台机器](/docs/explanation/tables)** — 存储了哪些信号，以及如何避免用另一台机器的数据作答。
- **[已退役的异常检测与诊断设计](/docs/explanation/evidence)** — 历史记录：排序结果的设计已被移除。请改用 SQL。
- **[已退役的诊断部署检测设计](/docs/explanation/deploys)** — 历史记录：自动部署检测已被移除。`service.version`
  仍可查询。
