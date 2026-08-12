---
title: sigiro 文档
description: sigiro 是自修复软件的检测与诊断层。输入 OpenTelemetry，输出排序后的结果和可运行的 SQL，面向智能体而非仪表盘。
sidebar:
  order: 0
---
sigiro 是面向智能体而非仪表盘的可观测性后端。它读取 OpenTelemetry，并回答自动化闭环在修复故障之前必须回答的两个问题：**发生了什么变化**，以及**为什么**。

本文档分为四个部分。每个部分回答不同类型的问题，请直接前往与你当前问题相匹配的部分。

## 教程

一堂课。请先阅读这一部分，因为它不预设你了解 sigiro。你按步骤操作，每一步都能看到结果。

- **[快速上手](/docs/tutorials/quickstart)** —— 运行服务端，向它发送遥测数据，然后询问它发生了什么变化以及为什么。整个过程需要五分钟。

## 操作指南

针对单个任务的操作说明。当你已经明确目标、只需要具体步骤时，请阅读这一部分。

- **[用智能体为你的代码埋点](/docs/how-to/install-with-ai)** —— 给智能体一条提示词。智能体会把 OpenTelemetry 接入你的服务，并将其指向 sigiro。
- **[向托管版 sigiro 发送遥测数据](/docs/how-to/hosted-onboarding)** —— 你自己无需运行任何服务端。使用租户密钥，把 OTLP 发送到我们运维的主机。
- **[将智能体自身的遥测数据发送到 sigiro](/docs/how-to/agent-telemetry)** —— 把 Claude Code 或 Codex 指向 sigiro，然后用 SQL 回读这些会话。

## 参考

关于这台机器的事实。在工作过程中阅读这一部分，用于确认某个名称、类型或限制。

- **[API 参考](/docs/reference)** —— 五个端点。每个端点都配有请求演练场。sigiro 根据服务端发布的同一份 `/openapi.json` 文档生成这些页面，因此页面内容始终与 API 保持一致。
- **[CLI 参考](/docs/reference/cli)** —— 七个子命令、每一个标志，以及它们读取的每一个环境变量。

读取 `/openapi.json` 的智能体无需我们提供其他任何信息。

## 原理说明

sigiro 为何这样运作。当你想了解背后的原因、代价和替代方案时，请阅读这一部分。你不需要终端。

- **[关于这些表，以及它们描述的是哪台机器](/docs/explanation/tables)** —— 为什么 `sigiro_metrics_*` 和 `sys_*` 回答的是关于不同机器的问题，以及如何避免得出一个自信却错误的答案。
- **[关于用证据取代仪表盘](/docs/explanation/evidence)** —— 为什么答案是一个排序列表，且每一行都带有可运行的查询。
- **[关于发布检测，以及你的 span 必须携带什么](/docs/explanation/deploys)** —— sigiro 读取一个属性来识别发布，当该属性缺失时，它不会给出任何结论。
