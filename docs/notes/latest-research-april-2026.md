---
title: "Latest Research Review: Self-Improving Investigation Agents (April 2026)"
created: 2026-04-01
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'comparison', 'competitor', 'data-model']
---

# Latest Research Review: Self-Improving Investigation Agents (April 2026)

## Question: Is our design the latest and greatest?

Short answer: Our design from the investigation architecture docs is GOOD but
MISSING 3 key advances from 2025-2026 research. Here's what's new and what to update.

---

## NEW PAPERS THAT MATTER (2025-2026)

### 1. Contextual Experience Replay (CER) — June 2025
**Paper:** arxiv.org/abs/2506.06698
**Key idea:** Training-FREE self-improvement within the context window.
Accumulates past experiences into a dynamic memory buffer. Synthesizes
relevant experiences at inference time. No retraining needed.

**What's different from our design:**
Our design stores raw trajectories and retrieves similar ones.
CER goes further — it SYNTHESIZES experiences into condensed, actionable
summaries that fit in-context. Instead of "here are 5 similar incidents,"
it produces "across similar incidents, the root cause was always X when Y."

**Action for Sigiro:** [[architecture/investigation.md|InvestigationBlock]] should include a `synthesized_context`
field — not just raw similar incidents, but a condensed synthesis.

### 2. ReasoningBank — September 2025
**Paper:** arxiv.org/abs/2509.25140
**Key idea:** Distills GENERALIZABLE reasoning strategies (not just facts)
from self-judged successful AND failed experiences. Agent retrieves relevant
reasoning patterns at test time and integrates new learnings back.

**What's different from our design:**
Our semantic memory stores extracted rules ("when X, do Y").
ReasoningBank stores REASONING STRATEGIES — HOW to think about a class
of problems, not just what to do. Example: "When diagnosing latency spikes,
first isolate whether it's a single endpoint or fleet-wide, because the
[[architecture/investigation.md|investigation]] path diverges completely at that point."

**Action for Sigiro:** sigiro_insights table should store REASONING STRATEGIES
alongside factual rules. Two types:
- Factual: "N+1 queries show as single-endpoint p95 spike + high DB count"
- Strategic: "Always check if [[designs/future/anomaly-view.md|anomaly]] is single-endpoint vs fleet-wide first"

### 3. Skill Augmented GRPO for Self-Evolution (SAGE) — December 2025
**Paper:** arxiv.org/abs/2512.17102
**Key idea:** Uses RL (specifically GRPO) to train agents to BETTER USE
their skill library. Not just "store skills" but "learn when and how to
apply skills optimally." Current skill libraries rely on LLM prompting
for skill selection — this uses RL to make selection more reliable.

**What's different from our design:**
Our design uses embedding similarity for skill matching (>0.85 threshold).
SAGE trains a policy that learns WHEN to use skills vs [[architecture/investigation.md|investigate]] fresh,
and WHICH skill to apply when multiple match. This is a genuine advance —
it means the skill selection itself improves with RL, not just the library.

**Action for Sigiro:** This is the RL angle we need. Instead of just
exporting trajectories for external training, Sigiro could:
1. Track skill selection accuracy (was the matched skill actually useful?)
2. Use GRPO to optimize skill selection policy
3. This is Phase 4 material but architecturally important now

### 4. Mobile-Agent-E — January 2025
**Paper:** arxiv.org/abs/2501.11733
**Key idea:** Hierarchical multi-agent with self-evolution through experience.
Manager agent decomposes tasks, worker agents execute, experience module
stores reusable "tips" extracted from both successes and failures.

**What's different from our design:**
We have a flat [[architecture/investigation.md|investigation]] → learn cycle. Mobile-Agent-E separates:
- MANAGER: high-level diagnosis strategy
- WORKERS: specific [[architecture/investigation.md|investigation]] queries
- EXPERIENCE: tips that improve both manager and workers

**Action for Sigiro:** Consider splitting [[architecture/investigation.md|InvestigationBlock]] generation:
- Strategy layer: "what kind of problem is this?" (managed by learned patterns)
- Execution layer: "run these specific queries" (managed by skills)
- This separation means strategies and skills evolve independently

### 5. EvoAgent — February 2025
**Paper:** arxiv.org/abs/2502.05907
**Key idea:** Self-evolving agent with a CONTINUAL WORLD MODEL that
autonomously updates through self-planning, self-control, and self-reflection.
Addresses catastrophic forgetting when facing new tasks.

**What's different from our design:**
We don't address CATASTROPHIC FORGETTING. When Sigiro learns a new pattern,
it might degrade performance on old patterns (e.g., new skill shadows a
better existing one). EvoAgent maintains a world model that prevents this.

**Action for Sigiro:** Skill library needs regression testing:
- When adding a new skill, replay past incidents to check it doesn't
  worsen accuracy on previously-solved patterns
- sigiro train --regression-check before promoting new skills

### 6. WebCoach — November 2025
**Paper:** arxiv.org/abs/2511.12997
**Key idea:** Self-evolving framework with PERSISTENT CROSS-SESSION MEMORY.
Three components:
1. WebCondenser: standardizes raw logs into structured experiences
2. Memory guidance: retrieves relevant past experiences across sessions
3. Continual learning WITHOUT retraining

**What's different from our design:**
WebCoach's "Condenser" is the key innovation — it standardizes messy
raw data (navigation logs) into a structured format BEFORE storing.
We do this somewhat ([[architecture/investigation.md|InvestigationBlock]] is structured), but we don't
explicitly condense the [[architecture/investigation.md|investigation]] trajectory into a reusable format.

**Action for Sigiro:** Add an explicit "condense" step after [[architecture/investigation.md|investigation]]:
- Raw trajectory: 9 queries + reasoning + outcome
- Condensed: { pattern, key_queries, diagnosis, confidence }
- This condensed form is what goes into episodic memory, not the raw trace

### 7. AIOpsLab — January 2025
**Paper:** arxiv.org/abs/2501.06706
**Key idea:** Holistic framework for evaluating AI agents in cloud operations.
Covers the ENTIRE incident lifecycle (detection → diagnosis → remediation).
Proposes benchmarks for comparing AIOps agents. Envisions "AgentOps" —
AI agents autonomously managing operational tasks.

**What's different from our design:**
We don't have an EVALUATION FRAMEWORK. How do we know our self-improvement
actually works? AIOpsLab defines:
- Fault injection scenarios (controlled testing)
- Scoring metrics (diagnosis accuracy, time-to-resolution, false positive rate)
- Benchmark comparisons across different agent approaches

**Action for Sigiro:**
- `sigiro train --benchmark` should implement AIOpsLab-style evaluation
- Controlled fault injection using historical data
- Track improvement over time with consistent metrics

### 8. "When AIOps Become AI Oops" — August 2025
**Paper:** arxiv.org/abs/2508.06394
**Key idea:** SECURITY ANALYSIS of AIOps systems. Shows adversaries can
MANIPULATE TELEMETRY to mislead AIOps agents into wrong actions.

**What's different from our design:**
WE DON'T ADDRESS ADVERSARIAL TELEMETRY. If someone can inject fake
metrics into Sigiro, the self-improvement loop amplifies the damage —
Sigiro learns from poisoned data and makes worse decisions over time.

**Action for Sigiro:**
- Telemetry integrity verification (checksums, source authentication)
- [[designs/future/anomaly-view.md|Anomaly]] detection on the TELEMETRY ITSELF (meta-anomalies)
- Skill promotion requires multiple independent confirmations
- This is a real risk, not theoretical — the paper demonstrates attacks

### 9. ROAD — December 2025
**Paper:** arxiv.org/abs/2512.24040
**Key idea:** Reflective Optimization via Automated Debugging. Optimizes
prompts WITHOUT gold-standard datasets by treating optimization as debugging.
Works from messy production logs and evolving failure modes.

**What's different from our design:**
Our DSPy optimization (Phase 3) assumes good training data. ROAD works
with messy, unlabeled production data — exactly what Sigiro has. It uses
reflection + automated debugging to improve prompts iteratively.

**Action for Sigiro:** Replace DSPy-based optimization with ROAD-style
approach for Phase 3. More realistic for production systems where you
don't have clean labeled datasets.

---

## WHAT OUR DESIGN GETS RIGHT (confirmed by 2025-2026 research)

1. **Four memory types (CoALA framework)** — Still the standard.
   ReasoningBank, WebCoach, Mobile-Agent-E all use variants of this.

2. **Skill library with embedding retrieval** — Confirmed by SAGE paper
   as the right foundation, but SAGE adds RL-based skill selection on top.

3. **Learning from both successes and failures** — Confirmed by ExpeL,
   ReasoningBank, Mobile-Agent-E. Contrastive learning is consensus.

4. **Agent-agnostic via MCP** — No paper contradicts this. Most frameworks
   are model-agnostic by design.

5. **Automatic reward signal from metrics** — Confirmed by AIOpsLab
   as a valid approach (metrics-based evaluation).

6. **Event-driven [[architecture/investigation.md|investigation]]** — Confirmed. No paper suggests
   always-on LLM monitoring.

---

## WHAT OUR DESIGN IS MISSING (gaps found)

### GAP 1: Experience SYNTHESIS, not just retrieval
**Papers:** CER (2506.06698), WebCoach (2511.12997)
**Issue:** We retrieve similar incidents. We should SYNTHESIZE them into
condensed actionable context. "Across 4 similar incidents, the pattern was..."
**Fix:** Add synthesis step in [[architecture/investigation.md|InvestigationBlock]] generation.
**Priority:** HIGH — directly reduces tokens and improves accuracy.

### GAP 2: Reasoning STRATEGIES, not just factual rules
**Papers:** ReasoningBank (2509.25140)
**Issue:** Our semantic memory stores "when X, do Y" rules. We're missing
"how to THINK about this class of problem" strategies.
**Fix:** Two types in sigiro_insights: factual rules + reasoning strategies.
**Priority:** HIGH — strategies transfer better across novel incidents.

### GAP 3: RL-optimized skill selection (SAGE)
**Papers:** SAGE (2512.17102)
**Issue:** We use embedding similarity for skill matching. This is fragile —
similar embeddings don't guarantee the right skill. RL can learn optimal
skill selection from feedback.
**Fix:** Track skill selection outcomes. Phase 4: GRPO for skill selection.
**Priority:** MEDIUM — embedding works initially, RL is an optimization.

### GAP 4: Catastrophic forgetting prevention
**Papers:** EvoAgent (2502.05907)
**Issue:** New skills might shadow better existing ones. No regression testing.
**Fix:** `sigiro train --regression-check` before promoting new skills.
**Priority:** MEDIUM — becomes important as skill library grows.

### GAP 5: Adversarial telemetry resistance
**Papers:** "AI Oops" (2508.06394)
**Issue:** Poisoned telemetry → poisoned learning → cascading bad decisions.
**Fix:** Telemetry integrity checks, multi-signal confirmation for skill
promotion, [[designs/future/anomaly-view.md|anomaly]] detection on telemetry patterns themselves.
**Priority:** LOW for indie devs, HIGH for enterprise.

### GAP 6: Evaluation framework
**Papers:** AIOpsLab (2501.06706)
**Issue:** No way to measure if self-improvement actually works.
**Fix:** `sigiro train --benchmark` with fault injection + scoring.
**Priority:** MEDIUM — needed before claiming "self-improving."

### GAP 7: Prompt optimization from messy data (ROAD)
**Papers:** ROAD (2512.24040)
**Issue:** DSPy optimization assumes clean datasets. Production data is messy.
**Fix:** Use ROAD-style reflective optimization instead of DSPy bootstrap.
**Priority:** LOW — only matters for Phase 3 (own-LLM mode).

---

## REVISED ARCHITECTURE (incorporating 2025-2026 research)

```
BEFORE INVESTIGATION:
  1. Match trigger → skill library (embedding similarity)        [existing]
  2. If match: RL-optimized skill selection (pick BEST skill)    [GAP 3 - SAGE]
  3. Retrieve similar episodes from episodic memory              [existing]
  4. SYNTHESIZE episodes into condensed context                  [GAP 1 - CER]
  5. Retrieve reasoning strategies for this problem class        [GAP 2 - ReasoningBank]
  6. Compose InvestigationBlock with all augmentations           [existing]

DURING INVESTIGATION:
  7. Hierarchical: strategy layer picks approach                 [GAP - Mobile-Agent-E]
  8. Execution layer runs targeted queries                       [existing]
  9. If novel: full 9-query investigation                        [existing]

AFTER INVESTIGATION:
  10. Record trajectory to episodic memory                       [existing]
  11. CONDENSE trajectory into reusable format                   [GAP 1 - WebCoach]
  12. Check resolution (auto from metrics + user feedback)       [existing]
  13. If confirmed + novel: create/update skill                  [existing]
  14. Regression check: new skill doesn't break old patterns     [GAP 4 - EvoAgent]
  15. Every N episodes: extract reasoning strategies              [GAP 2 - ReasoningBank]
  16. Every N episodes: synthesize experience summaries           [GAP 1 - CER]

PERIODIC (background):
  17. Update adaptive thresholds per service                     [existing]
  18. Decay stale skills (>90 days unused)                       [existing]
  19. Run benchmark on past incidents (improvement tracking)      [GAP 6 - AIOpsLab]
  20. Telemetry integrity checks                                 [GAP 5 - AI Oops]
```

## UPDATED IMPLEMENTATION PHASES

**Phase 0: Foundation** (unchanged)
- [[architecture/investigation.md|Investigation]] tracking table, record_outcome MCP tool

**Phase 1: Skill Learning + Experience Synthesis** (UPDATED)
- Skill library with embedding retrieval (existing [[company/plan.md|plan]])
- ADD: Experience CONDENSER (WebCoach pattern) — standardize trajectories
- ADD: Experience SYNTHESIZER (CER pattern) — synthesize, don't just retrieve
- ADD: Reasoning strategies in sigiro_insights (ReasoningBank pattern)

**Phase 2: Adaptive Thresholds + Evaluation** (UPDATED)
- Per-service learned thresholds (existing [[company/plan.md|plan]])
- ADD: Benchmark framework (AIOpsLab pattern)
- ADD: Regression checking for new skills (EvoAgent pattern)

**Phase 3: RL-Optimized Skill Selection** (NEW — replaces DSPy)
- GRPO for skill selection optimization (SAGE pattern)
- Track skill selection accuracy as reward signal
- Export training data for Atropos/TRL

**Phase 4: Advanced** (UPDATED)
- ROAD-style prompt optimization from messy data (replaces DSPy)
- Adversarial telemetry resistance
- Cross-user aggregate learning (opt-in, anonymized)

---

## HONEST ASSESSMENT

**What's genuinely novel in our design (not in any paper):**
- SQL-queryable telemetry as the self-improvement substrate
- Agent-agnostic via MCP (papers are all framework-specific)
- [[architecture/data-model.md|DuckDB]] as the unified store for telemetry + memory + skills
- 1-env-var install as the constraint that shapes everything

**What's well-established (we're applying known patterns):**
- Four memory types (CoALA, 2023)
- Skill library (Voyager, 2023 — enhanced by SAGE, 2025)
- Experience synthesis (CER, June 2025)
- Reasoning strategy memory (ReasoningBank, Sept 2025)
- Contrastive learning from success/failure (ExpeL, 2023)

**What's still cutting edge (limited evidence, high potential):**
- RL for skill selection (SAGE, Dec 2025 — one paper)
- ROAD for prompt optimization from messy data (Dec 2025 — one paper)
- Adversarial telemetry resistance (Aug 2025 — identified, not solved)

**Bottom line:** Our design was ~80% aligned with 2025 state of the art.
The 3 biggest gaps were: experience synthesis (not just retrieval),
reasoning strategies (not just factual rules), and regression testing
for skills. All fixable. The core architecture ([[architecture/data-model.md|DuckDB]] + MCP + 4 memory
types + automatic reward signal) is solid and confirmed by recent research.
