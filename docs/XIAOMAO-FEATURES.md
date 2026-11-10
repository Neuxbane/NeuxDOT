# XiaoMAO → NeuxDOT: harness diff and decisions

Source under review: a sibling `XiaoMAO` checkout (Go/NXP), cited as `XiaoMAO/<path>:<line>`.
Target: this repo, cited as `<path>:<line>` relative to the repo root.

| Short | Path |
|---|---|
| `EX` | [src/core/neuxdot-core/src/runtime/executor.rs](../src/core/neuxdot-core/src/runtime/executor.rs) |
| `MT` | [src/core/neuxdot-core/src/runtime/meta_tools.rs](../src/core/neuxdot-core/src/runtime/meta_tools.rs) |
| `TM` | [src/core/neuxdot-core/src/runtime/timer_manager.rs](../src/core/neuxdot-core/src/runtime/timer_manager.rs) |
| `SUB` | [src/core/neuxdot-server/src/subscriptions.rs](../src/core/neuxdot-server/src/subscriptions.rs) |
| `SC` | [src/core/neuxdot-core/src/db/schema.rs](../src/core/neuxdot-core/src/db/schema.rs) |
| `RP` | [src/core/neuxdot-core/src/db/repos.rs](../src/core/neuxdot-core/src/db/repos.rs) |
| `X` | `XiaoMAO/` (sibling checkout) |

Line numbers are anchored to the working tree at the time of writing; `executor.rs` was being edited concurrently, so each `EX` citation also names the function or branch it points into — re-grep the name, not the number.

---

## 1. Verdict

NeuxDOT already owns the three highest-value instincts in XiaoMAO and, in two of them, is materially better. **JIT tool discovery** is fully present (`MT:78,99,116`; dispatch at `EX:1486,1518,1543`) with a *separate* `get_tool_schema` call, which sidesteps the exact complaint XiaoMAO's own agent filed as C6 (full schemas on every `searchTools` match) — NeuxDOT never pays schema cost unless asked. **Self-describing truncation with read-back** is stronger than XiaoMAO's: NeuxDOT emits `<<truncated:FROM-TO>>` absolute *character* markers (`MT:304,354`) plus a trailing `[full content: history_inspect(id="…")]` handle (`EX:717-722`), backed by an append-only `agent_transcript` table (`SC:1103-1115`, written at `EX:654`) and paged by `history_inspect` (`MT:273` def, `EX:1626` impl; `RP:2071`). That is UTF-8-safe and self-locating; XiaoMAO's equivalent is a byte-slice `inspectIncoming` with a 5000-byte cap (`X/buffer.go:453,465`) whose receipts are *not* wired to it — the agent's own receipt preview is 160 chars with no continuation handle (`X/tools.go:1063-1070`). **Dual history** is present and cleaner: `agent_history` (working set, pruned) vs `agent_transcript` (immutable, per-turn, never pruned) at `SC:1088,1103`, which is exactly the "UI sees no compaction" property `X/idea.md:§1.C` argues for.

NeuxDOT is also ahead where the XiaoMAO post-mortems hurt most: **poisoned-history self-repair**. XiaoMAO wedged an agent permanently by persisting one malformed `image_url` and had to hand-repair the DB (`XiaoMAO/HANDOFF-inspectmedia-fix.md`, "Repair tooling" / "⚠️ BUG IN THE REPAIR PROGRAM"). NeuxDOT normalises and repairs history at every turn load (`EX:325` `normalize_history_transcript`, `EX:263` `append_missing_tool_responses`, `EX:110` `sanitize_chat_messages`), and has regression tests for the exact corrupted shape (`EX:2443-2467`). The **permission model** has no XiaoMAO counterpart: `PluginPermissions { elevated, network.allow, … }` (`SC:383-416`) is deny-by-default and manifest-declared; no XiaoMAO equivalent exists in the source I read: there is no per-plugin permission manifest under `XiaoMAO/define/` — "unclear in source" whether anything gates the sandbox beyond the shell tool's own args.

What NeuxDOT genuinely lacks is the **durable, non-blocking idle primitive**: XiaoMAO's `sleep` ends the turn and persists a one-shot wake (`X/tools.go:637`, `X/timer.go:32,46,60`, `X/store/wake.go:12-20` with explicit "one slot per agent / one-shot / set-once" invariants) and re-arms on boot (`X/timer.go:121` `RecoverTimers`). NeuxDOT's `sleep` **blocks inside the turn** (`EX:1987-2036`), and its timer manager is a process-local `HashMap` with **no persistence at all** (`TM:12-40`) — so a restart silently drops every pending wake. It also lacks: any unified error envelope, multi-node/mutable memory with delete, any event queue with inspect-by-id, `@data` zero-copy pipes, and an explicit post-prune notice to the model.

---

## 2. Feature table

| Feature | XiaoMAO mechanism | NeuxDOT today | Verdict | Effort |
|---|---|---|---|---|
| JIT tool discovery | `searchTools(query\|regex, detail=false)` returns compact rows by default, full schema only on exact name (`X/tools.go:646,773-818,822-827`) | `search_tools` → name/server/original/description only (`EX:1486-1517`); schema via separate `get_tool_schema` (`EX:1518`) | **have, better** (no schema tax; misses semantic search — NeuxDOT is substring-only, `EX:1491-1495`) | S to add regex/semantic |
| Self-describing truncation + read-back | `"... truncated 512/16182 bytes. Continue: inspectIncoming(id=…, from=…)"` (`X/buffer.go:499`) but receipts cut at 160 chars with no handle (`X/tools.go:1068-1070`) | `<<truncated:FROM-TO>>` char markers (`MT:304,354`) + `history_inspect(id,from,to)` handle appended (`EX:717-722`), transcript-backed (`EX:654`, `RP:2071`) | **have, better** | — |
| Dual history (audit vs working) | `running_history` (compacted) + event log for UI (`X/idea.md:§1.C`, `X/runner.go:315,398,410`) | `agent_history` (pruned, `RP:1803`) + immutable `agent_transcript` (`SC:1103`) with `origin`/`turn` | **have** | — |
| Model-controlled compaction | `handoff()` tool + auto handoff at threshold, **model-generated** state summary (`X/runner.go:410,619-648`; `X/tools.go:2139`) | `compact` tool (`MT:217`, `EX:1828`) rebuilds row 0 from current system prompt + memory; **automatic** path is a silent group prune (`EX:2159`), no summary, no notice | **partial** — manual yes, automatic is lossy and unannounced | M |
| Token-lean tool receipts | `id \| tool \| status \| bytes \| 160-char head` (`X/tools.go:1049-1083`); payload stays in queue | `call_tool` returns 1000+1000+1000 chars head/body/tail (`MT:299`, `EX:2110`); no per-call bytes/status receipt | **partial** — no receipt, default budget 18× XiaoMAO's preview | S |
| Cost visibility | `visible_bytes`/`total_bytes` per call (`X/tools.go:950-951`) | `ctx:N%` header on every tool message (`MT:414-430`, used `EX:2118-2122`) — a context gauge, not per-call bytes | **partial, different axis** | S |
| Unified error shape `{code,message,retryable,hint}` | Only 2 sites migrated (`X/tools.go:942`, `X/runner.go:601`); rest are strings | Absent; every branch is `{"error": "<string>"}` (`EX:1539,1589,1593,1598,1607,1619,1657`) | **missing in both** (NeuxDOT: 0 sites) | S |
| `@data` zero-copy pipes | `{{@data[id].payload.path\|filter}}`, hard error on unresolved, sha256 echo, refuses truncated stdout (`X/tools.go:2509,2563,2595,2697,2741-2750`; tests `X/agentrun_test.go:1938-1982`) | **absent** (no `@data` anywhere in `src/`) | **missing** | L (needs a queue store first) |
| Incoming event buffer + inspect-by-id | 5 s tumbling window (`X/buffer.go:74,666`), lean notice `{id,source,bytes}` (`X/buffer.go:121`), byte-slice `inspectIncoming` (`X/buffer.go:453`), `listIncoming` with `preview_chars` (`X/tools.go:1094-1118`) | `agent_pushed_messages` rows (`SC:1053`) injected once as a system row (`RP:2111`), listed via `GET /api/agents/:id/pushed` (`src/core/neuxdot-server/src/routes/agents.rs:378`); no coalescing, no byte-slice inspect | **partial** (durable yes, inspectable no) | M |
| sleep / timer as primitive | turn ends, wake persisted in `agents.next_wake_at`, boot re-arm (`X/tools.go:637`, `X/timer.go:32,46,121`, `X/store/wake.go:12-20`); rejects pure yield (`X/tools.go:2465-2485`) | **blocking** `tokio::select!` inside the turn (`EX:1987-2036`); process-local `HashMap`, no persistence (`TM:12-40`); rejects zero duration (`MT:15-70`) | **partial, worse** | S |
| Memory: keyed nodes + delete + search | per-key nodes, semantic+regex search, hash-guarded delete (`X/tools.go:2182,2211,2250,2288,2328`; `X/internal/store/memories.go:1314`) | single ≤5000-char blob, hash-optimistic `setMemory`/`editMemory`/`readMemory` (`MT:6,160-215`; `RP:1373`). **No delete, no search, no keys** | **partial, worse** | M |
| Scratchpad (notebook) with hash guard | `notebook_read/set/edit`, 12-char hash CAS, pre-loaded on handoff (`X/tools.go:2358-2450`; `X/runner.go:741`) | absent (memory blob is the only scratchpad, and it is context-window-replayed on every compact) | **missing** | S |
| Memory "lens" + coverage gate | summaries per key folded from an append-only log, gated by `tau_cover`/`tau_gain` cosine check (`X/internal/store/LENS_README.md §2`, `X/internal/store/lens.go:181`) | absent; memory is overwritten in place, no derivation, no staleness state | **missing** (high effort, unvalidated — see §5) | L |
| Delete primitives | memory delete + `PurgeIncomingStore` (`X/buffer.go:49`) | absent for memory, pushed messages, transcript | **missing** | S |
| Boot self-description (`GET /agent/self`) | proposed (D4), **not implemented** (`X/reports/REVISION — EASIER TO UNDERSTAND.md`; no route found in `server/`, `define/`) | tools injected as text via Handlebars `{{tools.preset}}`/`{{tools.bulleted}}` (`MT:443-517`); no self endpoint under `src/core/neuxdot-server/src/routes/` | **missing in both** | S |
| Invariant-enforcing tests | proposed (D5), not implemented (only the `builtinToolNames` slice, `X/tools.go:100`) | `test_meta_tools_schema_count` pins the exact 11-tool set and order (`MT:557-564`) | **have** | — |
| Freeform-text policy | hard ban + reprimand loop (`X/runner.go:488,711`) | opt-in per agent: `allow_output_text` (`SC:155`, enforced `EX:1331`) | **have, better** — see §5 | — |
| History self-repair | none; one poisoned turn wedged an agent until a hand-written DB repair (`X/HANDOFF-inspectmedia-fix.md`) | `sanitize_chat_messages` + `normalize_history_transcript` + synthetic tool replies (`EX:110,263,325`), with regression tests (`EX:2443-2467,2571`) | **have, better** | — |
| Media/upload tool | absent (M1, unfixed; agent hand-rolled MIME through `web_fetch`, `X/HANDOFF`) | `telegram_send_photo` exists but only URL/`file_id` (`built-in-plugin/telegram/src/server.rs:190,441`); no bytes upload | **missing in both** | — |

### 2.1 Where NeuxDOT is already ahead (say it plainly)

| Dimension | XiaoMAO | NeuxDOT | Why NeuxDOT wins |
|---|---|---|---|
| History recovery | Poisoned `image_url` persisted and replayed forever; repair was a hand-written Go program against a copied DB (`X/HANDOFF-inspectmedia-fix.md`, "Repair tooling") | `sanitize_chat_messages` / `normalize_history_transcript` run on every load, answer orphaned calls, and repair double-compaction (`EX:110,325,437-452`), with tests for each corrupted shape (`EX:2443-2571`) | Recovery is a property of the turn loop, not an incident response |
| Tool-result truncation | Byte slices, `stdout_truncated` refusals bolted on later (`X/tools.go:2741-2750`), previews with no handle | Character offsets, idempotent markers, continuation handle on the working-context copy (`MT:304,339,354`, `EX:717-722`) | UTF-8 safe by construction, and the marker *is* the instruction |
| Tool surface | All 18 primitives always advertised (`X/tools.go:100-120`), external tools behind one wrapper | 11 meta-tools always advertised (`MT:557-564`), external tools discovered then called (`EX:1486-1543`) | Smaller fixed schema, and `get_tool_schema` is a separate paid call — no C6 tax |
| Wake policy | Timer flush wakes agents unconditionally (`X/buffer.go:666-708`) | Wake is a predicate over payload shape (`SUB:44-56`) with non-wake regression tests (`SUB:452`) | Agents cannot be woken by their own outbound traffic |
| Permissions | No manifest-declared capability model found in source ("unclear in source") | `PluginPermissions { elevated, network.allow, … }`, deny-by-default (`SC:383-428`) | A wrong default here is a sandbox escape, not a token bill |

### 2.2 Every finding the agent filed, mapped

XiaoMAO's own post-mortems are the primary evidence; this is the status of each item against NeuxDOT. IDs are the agent's.

| ID | Finding (`X/reports/`) | NeuxDOT |
|---|---|---|
| C1 | Receipts 5–10× their information | **Open** — no receipt at all; budget is 3000 chars (`MT:299`) |
| C2 | Same bytes reported three times | **N/A** — no second report channel; `call_tool` returns the payload once (`EX:2110`) |
| C3 | Doubly-serialized inbound envelope | **Not applicable / unclear** — NeuxDOT's Telegram stream emits raw updates only (`built-in-plugin/telegram/src/server.rs:587-594`) |
| C4 | Duplicate pipe echo | **N/A** — no pipes |
| C5 | Previews on by default | **N/A** — no `listIncoming`; the analogous `GET …/pushed` is explicit and UI-facing (`agents.rs:378`) |
| C6 | Full schemas on every search match | **Fixed by design** — schema is a separate tool (`EX:1518`) |
| C7 | No cost visibility | **Partial** — `ctx:N%` per tool message (`MT:414-430`) is pressure, not per-call bytes |
| D1 | One error shape | **Open, wider surface** — 0 structured errors (`EX:1539,1589,1593,1598,1607,1619,1657`) |
| D2 | Self-describing truncation | **Better than the ask** — offsets + handle (`MT:304`, `EX:717-722`), except the event-stream copy (audit #2) |
| D3 | One schema per concept, `agent_id` on read | **Fixed** — normalised history + transcript scoped per agent (`EX:325`, `RP:2009,2071`) |
| D4 | Boot self-description | **Open** — no `GET /agent/self` route (`src/core/neuxdot-server/src/routes/`) |
| D5 | Enforce invariants in tests | **Have** — tool-set shape pinned (`MT:557-564`); history-shape tests (`EX:2443-2571`) |
| R14.1 | Pipe must fail loud | **Moot until pipes exist**, but the *principle* is violated in 4 places (audit #2,3,9 and §2.2 D1) |
| R14.2–14.5 | Escaping, read-back, type duality, size guard | **N/A** — no pipes; all four are preconditions in §5 item 2 |
| M1 | Media upload | **Open in both** — plugin accepts URL/`file_id` only (`telegram/src/server.rs:441`) |
| M2 | Delete primitives | **Open** — nothing deletes memory, pushes or transcript |
| M3–M4 | Cost endpoint, compact listing | **Moot** — NeuxDOT's listing is already compact (`EX:1486-1517`) |
| M5 | `sleep(until="next_event")` | **Blocked on #1** — without persistence and a queue it is a poll in disguise |

---

## 3. Silent-degradation audit (Q3)

The reports converge on "silent degradation is worse than loud failure" (`X/reports/WHAT'S MISSING.md`, final paragraph; `X/reports/CORRECTION FIRST.md`, last paragraph). **Yes — NeuxDOT has instances of the same shape, and one is a direct analogue of the false-receipt bug** (`FS TOOL FAMILY SILENTLY REBASES …`): the tool returns success for a write that did not happen.

| # | Site | What it does | Why it is the same disease |
|---|---|---|---|
| 1 | `EX:1715-1717` (`setMemory`) and `EX:1785-1787` (`editMemory`) | `let _ = self.db.set_agent_memory(...).await;` then replies `{"success": true, "updated_hash": …}` (`EX:1724-1728`, `EX:1794-1798`) and emits `MemoryUpdated` | The receipt confirms a durable write that was discarded. Next `readMemory` returns the old blob and the model concludes it misremembered. Direct analogue of XiaoMAO's `fs_write` receipt confirming a path the bytes are not at. |
| 2 | `EX:692-698` `truncate_event_payload` → `EX:2054` | Event-stream payloads are truncated with `response_id = None`, so **no** `[full content: history_inspect(…)]` handle is ever appended; the working-context copy at `EX:2110` does get one (`EX:717-722`) | Two consumers, one truncation contract. The UI/audit consumer sees a cut payload it cannot page, with no marker that a continuation exists. |
| 3 | `RP:2022` (`history_search`) | Invalid regex → `Err(_) => return Ok(Vec::new())`, so `EX:1619` makes the call succeed and returns `{"matches":[],"count":0}` with `success=true` | "No matches" and "your pattern did not compile" are indistinguishable — exactly R14.1's "never inject a value you are not certain about", applied to search. |
| 4 | `EX:2159` | `let _ = self.db.prune_agent_history_to_limit(...).await;` then the in-memory loop prunes regardless | A failed DB prune is invisible; the in-memory context and persisted history diverge, and the divergence only surfaces as unexplained context loss on the next turn. |
| 5 | `EX:1983-1993` | `interruptible` is parsed and put in the `AgentSleeping` event, but has no effect on the `select!` (every sleep is always cancellable, `EX:2007-2014`) | The schema advertises a behaviour (`MT:241-244`) that does not exist. A tool contract that lies is cheaper to fix than to discover. |
| 6 | `MT:454,504` (+ `MT:529,543`) | `hbs.set_strict_mode(false)`; on render error, `unwrap_or_else` falls back to a handful of literal `replace` calls. An unknown `{{variable}}` renders as **empty string, silently** | A system prompt can lose a variable and nothing reports it. The model then behaves against a truncated instruction set. |
| 7 | `SUB:358-369` → `SUB:393` | Push persist failure is `warn!`-only (`SUB:367-369`), then the agent is woken anyway (`SUB:393` spawns the turn); `EX:1192-1214` injects from the DB, so that event is absent from the turn's context | The wake happens, the event does not. Indistinguishable from "the plugin never sent anything". |
| 8 | `RP:2131-2134` + `EX:788-793` | Pushed-message batches are concatenated **uncapped** into a `system` row (the event-stream copy *is* truncated, `EX:952-965`) | One large push envelope blows the context budget; the safety net then silently deletes the oldest real history (`EX:2159-2202`). Loud alternative: truncate at injection and say so. |
| 9 | `EX:1430-1432` | `serde_json::from_str(fn_args_str).unwrap_or(json!({}))` — malformed tool arguments become an empty object, then the call is dispatched normally | The failure is re-attributed: the tool rejects for a missing `query`/`name`, so the model learns the wrong cause and retries the same broken JSON. XiaoMAO repaired this explicitly (`normalizeToolArgs`, `X/tools.go:35`: unwrap double-encoded JSON, compact whitespace) precisely because models emit it routinely. Here NeuxDOT converts a repairable input into a mislabelled error. |

Cheap bundle for 1, 3, 4, 7, 9: propagate the `Result`, return `{"error":{"code":…,"retryable":…,"hint":…}}` on failure, and repair (not discard) malformed `arguments` before dispatch. That is the same change as recommendation #2 below. Items 2, 6 and 8 need a per-site decision, not a shared helper.

---

## 4. Top 5 by value / effort

Ranked on (failure mode removed) ÷ (lines touched). Each is independently shippable.

**1. Persist the wake timer and re-arm it on boot — S.**
Change: add a nullable `agents.next_wake_at` (unix seconds) to `SC:143-166` and the idempotent block in `src/core/neuxdot-core/src/db/mod.rs`; `TM` gains `set_next_wake`/`clear_next_wake`/`armed_agents` over `RP`; the `sleep` branch (`EX:1987-2036`) writes the slot before arming and clears it on fire (`EX:2009-2029`); a boot sweep re-arms live slots and immediately fires stale ones (port of `X/timer.go:121-143`); keep the single-timer invariant already enforced at `TM:32-54`.
Removes: **restart silently discards every pending wake** — the agent is idle forever and nothing (log, event, row) says so. This is the failure mode XiaoMAO explicitly designed against (`X/store/wake.go:12-20`) and the one that most resembles "loudly broken looks like quietly fine".

**2. One error envelope, applied at every meta-tool branch — S/M.**
Change: a `ToolError { code, message, retryable, hint }` serializer in `MT`, used by all `Err` branches in `EX:1539,1589,1593,1598,1607,1619,1657` and by `append_missing_tool_responses`/`close_tool_call_group` synthetic replies (`EX:236,302,359`). Include the fix for audit item #3 (bad regex → `retryable:true, hint:"pattern did not compile"`).
Removes: **whole wasted turns**. XiaoMAO's D1 measured 4 turns lost to `Tool "" not found` and a first-call-fails-then-succeeds retry (`X/reports/REVISION — EASIER TO UNDERSTAND.md`, D1) — the precise class of error that teaches a model to distrust a correct call. NeuxDOT currently has zero structured errors, so it has strictly more surface than XiaoMAO had when the report was written.

**3. Per-call receipt + default truncation budget — S.**
Change: `call_tool` returns `{id, tool, status, bytes, head_160}` with the full payload reachable only via `history_inspect(id)` (`EX:2110-2132`); make the default `TruncationConfig` match the receipt (or expose it per agent) instead of 3000 chars (`MT:299`); add running `ctx_bytes` to the receipt. Keep the existing `head/body/tail` overrides (`EX:1549-1562`).
Removes: **5–10× tool-result token tax and an unfalsifiable "we saved tokens" claim** (`X/reports/REVISION — TOKEN BUDGET.md`, C1 and C7). This is the only change here whose payoff is a number.

**4. Memory: keys + delete (+ search) — M.**
Change: migrate the single blob (`SC:1080` `AgentMemory`, `RP:1373`) to keyed rows, keeping the hash CAS (`MT:8`) per key; add `deleteMemory` and `searchMemory` tools next to `MT:160-215`; render all keys into the prompt (already a Handlebars slot, `MT:483-501`). Port the guards from `X/internal/store/memories.go:1314` (stale-hash → explicit `STALE_HASH`).
Removes: **memory can only grow and only be overwritten wholesale**. There is no way to retract a stale fact; `editMemory` is blind search-replace (`EX:1738-1802`), and the 5000-char cap (`MT:6`) is a hard wall with no pruning story. XiaoMAO's `X/tools.go:2328` exists for exactly this.

**5. Automatic compaction must announce itself and carry a summary — M.**
Change: before the group-prune loop at `EX:2159-2202`, write a `system` row stating `pruned N tool groups (~M chars); recover via history_search`; when the turn is superseded or the prune exceeds a threshold, take the manual `compact` path (`EX:1828`) instead, which already rebuilds row 0 and archives the pre-state to the transcript (`EX:1913-1934`). Add the explicit "what you can no longer see" line to the notice.
Removes: **context silently disappears mid-task with no marker**. NeuxDOT is better than XiaoMAO here in *retrievability* (the transcript keeps everything, `RP:2009`), but worse in *disclosure*: NeuxDOT's automatic path emits nothing, while XiaoMAO at minimum injects `[CONTEXT BUDGET] used_chars=… level=…` notices (`X/runner.go:813-847`). The agent's own D2 principle — truncation must name its continuation — applies to compaction too.

Sequencing: #1 and #2 are independent and together remove the two failure modes a restart or a bad turn produces; #3 is independent of both and should ship in the same pass as #2 because a receipt *is* an error-shape neighbour (same response object). #4 must precede any pipe work; #5 must precede any increase in automatic-compaction aggressiveness.

Deferred deliberately, not forgotten: **`@data` pipes** and the **5 s tumbling event queue** (`X/tools.go:2509`, `X/buffer.go:74`). Each is L, and each depends on the other: a pipe without an addressable byte-queue is just string interpolation. NeuxDOT's `history_inspect` already covers read-back for the 80 % case (tool results), so the marginal value is event pipelining, not inspection. Do #1–#5 first; revisit pipes only with the escaping contract in §5 as a precondition.

---

## 5. Do NOT copy

**1. "No freeform text" as a hard rule (`X/idea.md:§1.B`, enforced `X/runner.go:488`).**
XiaoMAO hard-bans non-tool output and answers a violation with a reprimand `user` message injected into history (`X/runner.go:488-500`), then filters those synthetic turns out of the transcript (`X/runner.go:711`). Two costs: a full LLM round-trip per violation, and history rows that exist only to be special-cased later. NeuxDOT already has the correct shape — `allow_output_text` + `output_text_error_message` per agent (`SC:155`, `EX:1331-1343`). Keep it opt-in: shared bots and chat surfaces *must* be able to reply in text, and the per-agent flag lets the operator decide instead of the harness.

**2. `{{@data[…]}}` raw substitution, even in its post-fix form.**
The current Go source resolves pipes with hard failures and sha256 metadata (`X/tools.go:2615,2595,2697`) — better than the version the agent complained about — but the escaping contract R14.2 is still unmet: filters are only `raw|trim|json|b64` (`X/tools.go:2563-2593`), chosen by the *caller*, with no destination-aware escaping and no documented "exact-expression vs embedded-template" type duality (R14.4). That is precisely what produced the CRLF/multipart false bug and the 10-turn misdiagnosis (`X/reports/harness-revision-2026-10-10.md` §1, §2). If NeuxDOT ever ships pipes, the precondition is: unresolved path = hard abort naming the segment; **`|json` mandatory** for any insertion into a structured body; `inspectIncoming(id, "resolved")` returning byte-exact value + sha256 (R14.3); explicit size guard before dispatch (R14.5). Without those four, pipes convert a token saving into a silent-corruption engine.

**3. `sleep` as a turn-ender with no persistence — or as a blocker.**
Copying only half of XiaoMAO's sleep is what NeuxDOT effectively has today: the schedule is unpersisted (`TM:12-40`) while the call blocks the turn (`EX:1987`). If #1 lands, keep NeuxDOT's blocking form for short waits but stop advertising `interruptible` until it is honoured (`MT:241-244` vs `EX:2007-2014`). Do not adopt `sleep(until="next_event")` (M5) without an event source: `next_event` without a queue is `sleep(poll_interval)` wearing a different name (`X/reports/WHAT'S MISSING.md`, M5).

**4. A 5 s tumbling window that *starts runs* (`X/buffer.go:666-708`).**
XiaoMAO's flush loop spawns `StartRun` per agent per window on a bare timer, independent of whether the event is actionable. NeuxDOT's wake decision is a predicate, not a clock: `is_inbound_user_message` (`SUB:44-56`) plus a documented rationale for why the hub, not the executor, owns the subscription (`SUB:1-27`), with tests that outbound echoes do **not** wake (`SUB:452`). A timer-driven flush would wake agents on their own outbound traffic. If coalescing is added, add it to the *notice*, never to the *wake trigger*.

**5. Snapshot truncation with no handle (`X/tools.go:1952,1994,2021`).**
`payload[:1024] + " ...[truncated]"` in `searchHistory` is the exact anti-pattern the reports condemn in D2, still live in the source. NeuxDOT's offset-bearing markers (`MT:304,354` + `EX:717-722`) are strictly better; do not regress to bare ellipses anywhere (audit item #2 in §3 is the one place NeuxDOT still does this — fix it, don't spread it).

**6. Batch-invocation wrappers with self-banning rules (`X/tools.go:100-120`, `EX`-equivalent absent).**
`executeTools` must forbid the harness's own primitives inside itself (`X/tools.go:986-1010` builds a per-tool "cannot be nested" hint). NeuxDOT has no wrapper and needs none: native multi-call `tool_calls` already batch, and the executor is built to keep sibling replies adjacent (`EX:263`, `EX:1865-1892`). Adding a wrapper would create the nesting rule, not solve it.

**7. Lens-over-Log coverage gate — defer, do not adopt now.**
The gate (`tau_cover=0.60`, `tau_gain=0.00`, every tagged message must be covered, `X/internal/store/LENS_README.md:§2`) is genuinely interesting — a mechanical check on summary quality. But it requires an embedding sidecar (`X/internal/memory/embed.go`), per-message embeddings, tagging and link diffing: an L that buys nothing until keyed memory (#4) exists, and the reports contain **no measured evaluation** of whether the gate fires correctly (no false-pass/false-reject numbers). Adopt only with a measurement plan; a gate nobody has calibrated is another silent-degradation surface.

---

## 6. Evidence limits

What I could not establish from the source, stated rather than guessed:

- **`agent/self` (D4) and the D5 invariant test were never implemented in XiaoMAO.** I found no route under `XiaoMAO/server/` or `XiaoMAO/define/` and no assertion over `builtinToolNames` (`X/tools.go:100`). They are proposals, so they are compared as *ideas*, not as shipped mechanisms.
- **The `fs_*` path-rebase bug** (`X/reports/FS TOOL FAMILY SILENTLY REBASES ABSOLUTE PATHS ONTO HOME.md`) has no corresponding implementation in the repository I read: `fs_write`/`fs_stat` do not appear under `XiaoMAO/define/` or `internal/`. The report is treated as evidence of a *class* of defect, not as a feature comparison — I cite its shape, not its code.
- **`C3` (doubly-serialized inbound envelope)** is reported as measured against the Telegram stream, but the emitter lives outside the Go tree I can see; NeuxDOT's own `extension://telegram/updates` payload shape (`built-in-plugin/telegram/src/server.rs:587-594,614`) serializes raw updates only, so whether NeuxDOT ever had the 4× amplification is **unclear in source** and is not counted as a XiaoMAO advantage.
- **No measured evaluation of the lens coverage gate exists** in `X/reports/` — no false-accept / false-reject numbers. It is therefore deferred on evidence grounds, not on taste.
- **NeuxDOT token counts are unmeasured.** This document argues cost from configuration (`MT:299` default budget, receipt absence), never from a profiled session. If the author wants the C1 payoff to be actionable, the first artefact to produce is a token histogram per tool call, not a refactor.
