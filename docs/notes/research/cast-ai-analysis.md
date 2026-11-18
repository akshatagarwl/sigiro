---
title: Can Sigiro Deliver Cast.ai's Value Better and Cheaper?
created: 2026-03-24
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'data-model', 'duckdb', 'ebpf', 'logs', 'market']
---

# Can Sigiro Deliver Cast.ai's Value Better and Cheaper?

## What Cast.ai Actually Sells

Cast.ai charges **15%+ of identified savings** (not actual savings — what they
*identify*). Their [[designs/pricing.md|pricing]] is sales-led, opaque at enterprise level.

What they do:
1. Install [[roadmap.md|Helm]]-based agents in your K8s cluster
2. Agent sends cluster metadata to their SaaS engine
3. Engine runs bin-packing + [[company/cost.md|cost]] optimization algorithms
4. Automated actions: rightsizing pods, scaling/replacing nodes, spot management
5. Security via kvisor ([[designs/future/ebpf-gpu-position.md|eBPF]]-based runtime monitoring)

Components: castai-agent, cluster-controller, evictor, spot-handler, kvisor,
workload-autoscaler, pod-pinner, audit-logs-receiver.

## Their Weaknesses (from real users)

1. **K8s-only** — doesn't touch VMs, serverless, ECS, anything non-K8s
2. **Compute-only** — doesn't optimize storage, networking, data transfer
3. **Karpenter is eating them alive** — free, open-source, handles most of what
   they do on AWS/EKS
4. **Savings diminish but fees don't** — 15% of savings that plateau over time
5. **Production instability** — agent conflicts (cert-manager), aggressive defaults
6. **Vendor lock-in** — deep cluster access, dependency on their SaaS uptime
7. **No OTel** — proprietary collection, not aligned with open standards
8. **Misleading marketing** — "cut in half" but actual range is 12-70%

## Where Sigiro Can Attack

### The Key Insight

Cast.ai works at the **infrastructure layer** (nodes, pods, instance types).
They don't understand what your application is *doing*. They see CPU/memory
numbers, not request traces, error chains, or code-level bottlenecks.

Sigiro works at the **application layer** (traces, spans, code paths). This is
fundamentally more valuable because:

- Infra optimization is becoming commoditized (Karpenter, native autoscalers)
- App-level understanding is where the unsolved problems live
- AI coding agents need *code context*, not cluster metadata

### Concrete Opportunities

**1. "Observability-driven rightsizing" — better than blind bin-packing**

Cast.ai rightsizes based on historical CPU/memory metrics. That's a blunt
instrument. Sigiro, with trace data, can understand:
- This service is slow because it's making 47 sequential DB calls, not because
  it needs more CPU
- This OOM is from a memory leak in the handler, not from under-provisioning
- This pod is idle 90% of the time because it only handles batch jobs at 2am

You can provide SMARTER rightsizing recommendations because you understand WHY
resources are consumed, not just HOW MUCH.

**2. [[company/cost.md|Cost]] attribution that actually means something**

Cast.ai's [[company/cost.md|cost]] attribution is at the namespace/pod level. Users complain it's
weak. Sigiro can tie costs to *business operations*:
- "This API endpoint costs $47/day because of N+1 queries"
- "Your checkout flow costs 3x more than login because of payment retries"
- Code-level [[company/cost.md|cost]] attribution > infra-level cost attribution

**3. Not K8s-only — works everywhere**

Sigiro uses [[architecture/write-path.md|OpenTelemetry]], which works on K8s, VMs, serverless, bare metal,
edge, whatever. Cast.ai is locked to K8s. The 50%+ of workloads NOT on K8s are
an untapped market for observability-driven optimization.

**4. Agent-native remediation**

Cast.ai's "self-healing" is pattern-matched K8s actions (restart pod, scale
node, replace instance). Sigiro's angle is AI agent remediation:
- Agent sees trace showing slow response → identifies root cause in code →
  suggests/applies fix
- This is fundamentally more powerful than "add more nodes"
- Cast.ai can't fix your code; Sigiro + AI agents can

**5. Radically cheaper**

Cast.ai takes 15%+ of savings. If you save $200K/year, you pay them $30K+.

Sigiro's cost model (Rust + [[architecture/data-model.md|DuckDB]], scale-to-zero on Fly):
- Near-zero marginal [[company/cost.md|cost]] per trace ingested
- No SaaS engine running 24/7 per customer
- Could charge flat fee ($99-299/mo) or usage-based (per trace/span)
- Even at $299/mo ($3,588/yr) you'd be 10x cheaper than Cast.ai for a
  mid-size customer

## The Play: "Observability That Optimizes"

Don't try to BE Cast.ai. Instead, build the layer Cast.ai can't touch:

```
Cast.ai world:         cluster metrics → bin-packing → node changes
Sigiro world:          app traces → root cause analysis → code fixes + infra recs
```

Phase 1 (now): App-level observability for AI coding agents
Phase 2 (next): "Your traces show you're wasting $X/mo — here's why and how to fix it"
Phase 3 (later): Auto-remediation via AI agents using trace context

The "better and cheaper" [[company/pitch.md|pitch]] isn't "we do what Cast.ai does for less" — it's
"we solve the problem Cast.ai can't even see, and [[company/cost.md|cost]] optimization falls out
as a side effect."

## Honest Assessment

Can Sigiro replace Cast.ai today? **No.** Cast.ai has node-level autoscaling,
spot management, and bin-packing that requires deep cloud provider integration.
Building that is a massive engineering effort and it's being commoditized anyway.

Can Sigiro provide MORE value than Cast.ai for a developer team? **Yes** —
because knowing *why* your app is slow/expensive is more valuable than
auto-shuffling pods across cheaper nodes. The code fix that eliminates 47
sequential DB calls saves more than any amount of instance rightsizing.

Can Sigiro be 10x cheaper? **Easily** — Rust + [[architecture/data-model.md|DuckDB]] + scale-to-zero vs
their SaaS engine + percentage-of-savings model.

## Bottom Line

Don't compete at the infra layer. Own the app layer. The positioning is:

"Cast.ai optimizes your cluster. Sigiro optimizes your code.
One saves you 30% on nodes. The other eliminates the 10x
inefficiency in your application that's burning those nodes."
