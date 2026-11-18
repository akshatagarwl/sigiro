---
title: Agent Self-Registration Pattern
created: 2026-04-10
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'design', 'fly-io']
---

# Agent Self-Registration Pattern

**Source:** Browser-Use (browser-use.com) — observed 2026-04-09
**[[plans/STATUS.md|Status]]:** Idea / to evaluate for Sigiro

## The Pattern

An AI agent provisions its own API key autonomously — no human sign-up flow needed.

### How Browser-Use Does It

1. **Request a challenge**
   ```
   browser-use cloud signup
   ```
   Returns a Challenge ID + obfuscated math problem that requires LLM reasoning to solve.

2. **Solve and verify**
   ```
   browser-use cloud signup --verify <challenge-id> <answer>
   ```
   The agent solves the math, submits the answer (string with 2 decimal places), and the CLI saves the API key to `~/.browser-use/config.json`.

3. **Optional: Claim the account**
   ```
   browser-use cloud signup --claim
   ```
   Returns a claim URL valid for 1 hour. The human can follow it to sign up and take ownership of the agent-created account.

### Why This Is Smart

- **Zero friction**: Agent starts using the API immediately, no waiting for human to create an account
- **Anti-bot via LLM challenge**: The math problem is trivial for LLMs but hard for simple bots — clever gate
- **Deferred ownership**: Human can claim later, so you don't lose the "real user" conversion path
- **Agent-native UX**: Designed for machines, not humans — no browser sign-up, no email verification

## Sigiro Application

Sigiro is agent-facing infrastructure. Consider this pattern for:

- **API key provisioning**: Let agents (Hermes, Claude Code, Codex, etc.) self-register
- **Free tier onboarding**: Agent gets a limited free tier immediately, human upgrades later
- **Claim flow**: Deferred human ownership via claim URL or email link

### Design Considerations

- Challenge should be trivially solvable by any capable LLM (math, logic puzzle, etc.)
- Free tier should be generous enough to demonstrate value
- Claim URL should be short-lived but renewable
- Track agent-created vs human-created accounts separately for analytics
- Consider: what happens if the same "agent" (same codebase) registers from multiple machines?
