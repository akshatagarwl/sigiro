Commit and push using conventional commit guidelines after every logical change, build and test often.
Whenever you encounter any error, first try to figure out the history of the error instead of the just first fixing the error itself.
Always flag DI patterns.
Always follow Zig Zen principles.
Communicate intent precisely.
Handle edge cases.
Favor reading code over writing code.
Provide only one obvious way.
Prefer runtime crashes over bugs.
Prefer compile errors over runtime crashes.
Make incremental improvements.
Avoid local maximums.
Do not preserve backwards compatibility unless the user explicitly asks for it.
Significant architectural changes are acceptable because sigiro is a greenfield project.
Minimize what one must remember.
Focus on code rather than style.
Succeed at resource deallocation.
Use sigiro to debug sigiro.
No unnecessary abstractions; mark duplication for later review.
No overengineering; wrong abstractions are worse than none.
Stay under 5000 lines; extract crates when exceeded.
Reuse external crates; never reimplement the wheel.
Never drop errors silently; always fail loudly.
Always do E2E real user tests as extensively as possible, never unit tests.
Buy or reuse external packages; never reimplement the wheel.
After every turn ask is this an overabstraction or overengineering and remove it if so.
For any non-trivial task, research the codebase first, then present the plan and align with me on it, then orchestrate the work across parallel subagents; never ask three questions in place of a plan.

# Learning more about Effect

`agents/` uses the Effect TypeScript library.

Before writing any Effect code, first read `agents/node_modules/effect/AGENTS.md`
**completely**, and follow the links in the file when required.

If you need to learn more about particular Effect apis and concepts that the
guide doesn't cover, search through the source code in
`agents/node_modules/effect/src`.
