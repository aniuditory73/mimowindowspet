# Master prompt for Codex — MIMO MVP

You are the lead Windows/Rust engineer and product-minded architect for **MIMO**. Work inside the repository provided to you. Read `AGENTS.md` first and treat it as binding project guidance. Communicate with me in Russian.

## Product vision
Create a free, open-source, lightweight Windows 10/11 desktop assistant that sits unobtrusively on screen as a small dark pill/island. On click it expands into a compact utility panel. It performs small local tasks directly and can delegate complex, especially programming-related, work to my already installed/authenticated Codex CLI. This is a **functional productivity tool first**, not a virtual pet or a chat app. Visual polish, expressions, character, themes and advanced animations are deferred until the functional MVP is stable.

The core value proposition: “I state an outcome; MIMO chooses the safest available method, shows the task state and performs allowed operations without unnecessary confirmations.” However, maximum automation is limited by explicit permissions, reversible actions, bounded scope and clear approval gates.

## Technology and deployment
- Rust stable, Slint UI, Windows 10/11, x64 first; plan for Windows 11 and high-DPI monitors.
- Native desktop executable, local persistence in per-user application data, no admin rights, no mandatory server, no telemetry.
- No mandatory paid APIs. Offline local capabilities must work without Codex. Cloud-backed Codex depends on the user's own supported CLI authentication and applicable service limits; never promise unlimited free usage.
- Minimize dependencies and idle CPU/RAM usage. Define measurable performance budgets after first profiling rather than inventing benchmarks.
- Keep configuration and user data portable/migratable. Include license recommendation (e.g. MIT or Apache-2.0) for my approval; do not assume third-party assets are redistributable.

## Initial user experience
1. On launch, show a small black/dark pill near the top or chosen screen corner. Allow dragging or selecting a corner. Persist its position safely across monitor changes.
2. Click pill -> expand into a compact rounded panel; click close/collapse -> return to pill. Use simple, quick transition if easy; correctness matters more than animation.
3. Panel: command input; submit action; 3–5 quick actions; current task with status and cancel; concise result/history; settings entry.
4. Settings: approved directories, Codex CLI availability/auth status, selected Codex workspace, safe-mode/approval preferences (without disabling non-negotiable gates), startup behavior and hotkey when implemented.
5. Background work must not freeze UI. Show queued/running/awaiting approval/success/failure/cancelled states accurately.
6. Provide system tray exit and a clear way to reopen MIMO; avoid an always-on-top window covering fullscreen applications or stealing focus.
7. UI language Russian by default; architecture must permit localization. Use fonts available on Windows; do not ship proprietary Apple fonts.

## Functional features and examples
### A. Deterministic local operations
- Notes: create, list, open and search short notes in local storage. Example: “Запомни: оплатить интернет в пятницу”. Do not claim reminders are scheduled until a reminder subsystem actually exists.
- Files: inspect user-selected approved folders; preview classification by extension/type; move into categories; safe rename; collision detection; dry run; undo journal when feasible. Example: “Разложи изображения и документы из Загрузок по папкам, остальное не трогай”. If destination is outside approved roots, request approval to add it; do not silently expand access.
- Search: scoped filename/path search only in approved directories. Do not index entire disk by default.
- App launcher (later): only explicit allowlisted installed applications; no arbitrary command injection.
- Selected text/clipboard (later): translate, summarize, correct; ask before transmitting private text to remote AI. Do not implement persistent clipboard surveillance.

### B. Codex delegation
- Detect installed Codex CLI, read its version and available documented flags, show meaningful setup errors.
- Reuse supported CLI authentication; MIMO must not collect passwords, tokens or cookies.
- User selects an explicit project workspace. Example: “Найди и исправь ошибку в этом Rust-проекте”.
- Run Codex via a safe adapter: argument arrays (no shell string interpolation), restricted working directory, captured structured output if supported, exit status, cancellation, timeout, no unrestricted bypass flags.
- Make external execution and its workspace obvious. Never silently allow Codex to access unrelated personal folders or to modify the system.
- Display summary and changed files when reliably available. Distinguish model claims from verified file diffs and test results.
- Handle missing installation, authentication required, rate/quota limitations, network failure, crash, malformed output, version mismatch and user cancellation.

### C. Routing
- Prefer local typed tools for deterministic tasks; do not pay for model calls to create a note or list files.
- Use constrained intent recognition for a small supported command set. Do not pretend arbitrary Russian natural language is fully understood without an LLM.
- For unclear intent, ask one precise clarification; for unsupported tasks, explain limitations and optionally suggest a Codex task with explicit user approval.
- All operations pass the same central policy checker, regardless of source (UI, local router, Codex adapter).

## Permission and safety design
Build a concrete policy matrix and implement it in one central module:
- Auto: scoped read, create note, preview/dry run, approved reversible move with no overwrite, other explicitly allowlisted low-risk operations.
- Approval required: deletion, overwrite, irreversible bulk changes, moving outside approved roots, launching unknown processes, network transfer of local content, potentially destructive Codex operations.
- Denied in MVP: elevation, registry/system configuration modification, credential extraction, unrestricted shell execution, hidden monitoring, unauthorized remote control.
- Path handling: canonicalization, traversal and symlink/junction containment, race considerations, Unicode paths, file locks, permission errors, duplicate names, conflict policy, rollback/undo limitations.
- Approval dialog must identify exact affected files/locations and action; approval should be scoped to that task, not permanent blanket permission.
- Codex is an external process and its sandbox behavior varies by version/platform; MIMO cannot claim its policy gates fully mediate Codex's internal filesystem writes. Use the most restrictive supported CLI settings, narrow workspace, and document residual risks. Do not treat prompt instructions as a security sandbox.
- Log actions and failures without recording sensitive full content or secrets. No silent deletion.

## Proposed modular architecture (adapt to repository after inspection)
- `app`: startup, lifecycle, dependency wiring.
- `ui`: Slint views and Rust view-model bridge; no blocking work.
- `domain`: typed commands, task lifecycle, result models.
- `router`: constrained intent routing and clarification.
- `tasks`: queue, bounded concurrency, cancellation and progress events.
- `policy`: permission rules and approval decisions.
- `tools/files`: scoped file inspection, classification, preview, move/rename, undo.
- `tools/notes`: local note operations.
- `codex`: executable discovery, version capabilities, subprocess lifecycle, output parser.
- `storage`: config, notes, task metadata and migration.
- `platform/windows`: tray, positioning, hotkey, startup integration when appropriate.
Use names consistent with Rust conventions; these are conceptual modules, not a rigid folder prescription.

## Suggested UI layout
Collapsed: compact dark pill, no border required, no pet face yet.
Expanded: title/status row; command input and submit; quick actions (Files, Notes, Codex); active task card with cancel; last few operations; settings.
Use restrained rounded corners, neutral palette, readable typography, smooth but brief expansion if technically stable. Avoid copying proprietary UI assets. Keep panel compact, resizable only if needed, and usable with keyboard. Respect Windows scaling and multi-monitor setups.

## Implementation sequence and acceptance gates
**Phase 0 — audit and plan:** inspect current repository; identify existing scaffolding, compile risks and missing requirements; propose dependency choices and a short architecture decision record. Do not replace working code wholesale.

**Phase 1 — reliable shell:** buildable Windows Rust/Slint app, pill/panel toggle, tray exit, task state display, settings persistence, nonblocking background task runner and cancellation. Verify app remains responsive during simulated long task.

**Phase 2 — safety and local utility:** approved-root management, centralized policy, notes CRUD, scoped folder listing, file classification preview, safe non-overwriting moves and journal/undo where feasible. Test path escapes, collisions, locked files and denied operations.

**Phase 3 — Codex integration:** CLI detection, capability/version check, workspace selection, safe process launch, progress/log streaming, cancellation, errors and clear user-facing setup guidance. Test with fake executable before opt-in real Codex smoke test.

**Phase 4 — productivity polish:** hotkey, basic selected-text actions with explicit consent, history/search, packaging and Windows QA. Only after earlier phases are stable.

**Phase 5 — later, not MVP:** pet character, eyes/emotions, advanced elastic animations, customizable themes and positions, extensible skills. Design interfaces to support this later without implementing it now.

For every phase provide: planned changes, files modified, commands/tests run, observed results, known limitations and a recommended next step. Do not mark a phase complete on code generation alone.

## Required test scenarios
- Launch/collapse/reopen/exit on Windows; taskbar, fullscreen, multiple monitors and DPI behavior.
- Missing Codex CLI, unsupported version, unauthenticated CLI, offline, quota error, cancellation, process crash.
- Empty folder, large folder, nested paths, non-ASCII names, duplicate destination, locked file, denied folder, symlink/junction escape, file modified during operation.
- Unknown command, ambiguous intent, approval denied, app closed during running task, restart after interrupted task.
- Cancellation must not claim rollback unless verified. File moves must not overwrite without approval. No network access for local-only tasks.
- Unit tests for policy/router/task state; integration tests with disposable temp fixtures; fake CLI process tests; manual Windows smoke-test checklist.

## Deliverables
1. Buildable source repository with readable module structure.
2. `README.md` with requirements, installation, run/build commands, Codex setup, limitations and security model.
3. Example safe configuration and explanation of approved folders.
4. Tests and manual QA checklist.
5. A short architecture document and task-by-task progress checklist.
6. Windows executable/release packaging only after compilation and validation on Windows; do not falsely claim signed binaries.
7. Clear licensing and third-party dependency inventory.

## How to begin
**Do not implement everything at once.** First read `AGENTS.md`, inspect the existing repository and tell me in Russian:
1. What already exists and what needs correction.
2. Proposed architecture and dependencies with rationale.
3. A phased roadmap and acceptance criteria.
4. Key security and Codex CLI compatibility risks.
5. The smallest first implementation slice.

Then proceed with Phase 1 in small, testable changes unless you encounter a decision that requires my approval. Do not spend time on pet visuals yet. Never silently broaden filesystem permissions, disable approvals or invent successful test results.
