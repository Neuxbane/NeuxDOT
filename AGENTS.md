# AGENTS.md

Instructions for AI coding agents (and humans) contributing to NeuxDOT.

The goal of this file is narrow: **prevent dead code, security regressions, and
silent breakage**. It is not a style guide. When a rule here conflicts with your
instinct to "helpfully" refactor, the rule wins.

---

## 0. Ground rules

1. **Read before writing.** Open the file you intend to change and the one or two
   callers it has. Guessing at an API's shape is the single largest source of
   broken patches in this repo.
2. **Never invent an API.** If a route, repo method, or column does not exist,
   say so — do not add a speculative stub "for later". Unused code is a defect.
3. **Verify, don't assume.** Run the checks in §7 before claiming something works.
   "Should work" is not a result.
4. **Smallest correct change.** Do not reformat untouched code, rename unrelated
   symbols, or bundle opportunistic refactors into a fix.
5. **If you are wrong, say so plainly** and correct it. Do not quietly paper over
   a failed approach.

---

## 1. Architecture map

```
src/core/neuxdot-core/   runtime, DB, MCP transports
  src/db/                schema.rs (types), repos.rs (all SQL), mod.rs (init + migrations)
  src/runtime/           executor.rs, meta_tools.rs, router.rs, timer_manager.rs, media.rs
  src/mcp/               stdio, http (SSE), custom_tools, sandbox, proxy
  src/auth/              JWT, password hashing
src/core/neuxdot-server/ Axum HTTP API
  src/routes/            one module per resource
  src/middleware.rs      AuthUser, require_root, resolve_identity
src/apps/desktop/        desktop shell (Tauri v2)
src/apps/web/            SvelteKit 5 (runes) + Tailwind v4
built-in-plugin/         chat, computer, gitlab, telegram, discord (own crates)
docs/HOST_BRIDGE.md      sandboxed MCP-App ↔ host contract — treat as a spec
```

Two facts that trip people up constantly:

- `built-in-plugin/*` are **separate crates** with their own lockfiles. They are
  not workspace members; build them with `./compile.sh --plugins`.
- WASM/Wasmtime is **not implemented**. The `extensions` table and `wasm`
  transport name are scaffolding only; do not document or rely on a WASM runner.

---

## 2. Security invariants — never weaken these

These are the load-bearing walls. Changing any of them requires an explicit,
written rationale in the PR.

### 2.1 Authorization is DB-authoritative

`AuthUser` carries `account_id`, `username`, `role`. The role **must** come from
`resolve_identity()` (a DB lookup), never from JWT claims alone — a token can be
up to 30 days old and a demoted user would otherwise keep root.

```rust
// WRONG: trusts a stale claim
if claims.role == ROOT { ... }

// RIGHT: already resolved from the DB by the middleware
auth.require_root()?;
```

### 2.2 Never gate on `account_id` equality alone

Agents can be **shared**. Capability checks go through the access resolver:

```rust
let (agent, access) = load_agent_with_access(&state, &id, &auth.account_id).await?;
require_edit(access)?;              // or require_manage_shares(access)?
```

- View/use → `access.can_view()`
- Mutate config → `access.can_edit()`
- Change sharing → `access.can_manage_shares()`
- **Delete → owner only.** A sharee may leave, never destroy.

Regression test for this: `crates/neuxdot-server/tests/agent_sharing_test.rs`.

### 2.3 404 for foreign resources, not 403

For resources belonging to another account where existence itself is sensitive,
return `404`. `403` confirms the id is real and enables enumeration.

### 2.4 Sandbox trust boundary

- A plugin may **never** be trusted for identity. The host stamps
  `_sender_account_id` onto `tools/call` arguments; plugins read that, never a
  client-supplied value.
- WASM has no syscalls, no network, no clock. It reaches the outside world only
  through declared host functions, gated by manifest permissions.
- Any new capability added to `Capability`/`PluginPermissions` must be:
  1. deny-by-default,
  2. advertised only when granted (`PluginPermissions::capabilities()`),
  3. checked host-side before use.

### 2.5 SQL

- `repos.rs` is the **only** place that executes SQL. Routes and runtime call
  repo methods.
- Always bind parameters. String-formatted SQL is an injection bug.
- Migrations live in `db/mod.rs` and must be **idempotent** (`CREATE TABLE IF NOT
  EXISTS`, guarded `ALTER TABLE`). Startup runs them against live databases.

  ⚠️ The schema is split naively on `;`. **Never put a semicolon or an
  apostrophe (`'`, as in `agent's`) inside a SQL comment** — it silently
  corrupts the statement split and breaks every database init.

### 2.6 Secrets

Never log or return: password hashes, JWTs, API keys, `headers_or_env` contents.
`.secrets/`, `*.jks`, `*.keystore` are git-ignored — keep it that way.

---

## 3. No dead code, no unverified claims

### 3.1 Delete, don't stub

If a function is no longer used, **remove it**. Do not leave:

- `_unused` parameters kept "for compatibility",
- `// TODO: implement` bodies that return a fake value,
- duplicate helpers that shadow an existing one.

If the compiler warns `never used` / `unused variable`, fix the cause. The
workspace currently builds with **zero warnings**; keep it that way.

### 3.2 Check before you add

Before writing a helper, search for it:

```bash
rg -n "fn <name>|struct <Name>|enum <Name>" crates/ apps/web/src/
```

Duplicated logic is a defect even when it compiles.

### 3.3 No silent failure

```rust
// WRONG: hides the bug forever
let _ = state.db.something().await;
foo().unwrap_or_default();          // only if empty really is correct

// RIGHT: propagate, or log with context
state.db.something().await.map_err(internal_error)?;
```

`unwrap()` / `expect()` in request paths is forbidden. They are acceptable in
tests and in `main()` at startup, where failing loudly is correct.

---

## 4. Rust conventions

- Errors: `Result<T, String>` inside the runtime, Axum `(StatusCode, Json)` at the
  boundary via `internal_error`. A panic must never reach a user request.
- The `?` operator over `unwrap`; `map_err(internal_error)?` for DB calls.
- Prefer borrowing (`&str`, `&[u8]`) in internal signatures; own at boundaries.
- Types live in `db/schema.rs`; behaviour is exposed through `Database` methods in
  `repos.rs`.
- When adding a field to a struct that is `Deserialize`d from older rows, give it
  `#[serde(default)]` so existing data keeps parsing.

### 4.1 LLM protocol invariants (the fragile part)

The executor has one job that is easy to break: **an assistant message with
`tool_calls` must be immediately followed by exactly one `tool` message per
`tool_call_id`.**

- Never drop a sibling tool reply (multi-call turns must stay grouped).
- History writes must be atomic — use `replace_agent_history`, not a loop of
  appends.
- Compaction writes rows in the order: `[base][assistant][tool][summary]`.
- Pruning must be group-aware; never split an assistant/tool group.

Breaking this produces the production error
`insufficient tool messages following tool_calls message`. There are tests for
it — do not delete them.

### 4.2 Cache stability

The prompt prefix is cached by the provider. **Do not mutate a running agent's
history** outside compaction, and do not inject changing fields into the system
prompt per turn. Instruction changes are applied only at `compact`.

---

## 5. Frontend (SvelteKit 5)

- Run es only (`$state`, `$derived`, `$effect`). No `export let`, no stores for
  component-local state.
- **Read state lazily in `$effect`.** Reading a reactive value only to compute a
  key can cause spurious reloads or infinite loops.
- If a cached thing depends on server content, **key it on a version**
  (e.g. `wasm_hash`), not just on ids — otherwise updated assets never reload.
- A sandboxed MCP-App iframe has no network and no token. Everything goes through
  the host bridge (`hostCall`), and bridge results are wrapped `{ ok, value }` —
  **unwrap `.value`**.
- Accessibility: interactive controls get a real `<button>`, an accessible name,
  and keyboard handling.

---

## 6. Writing migrations & schema changes

Checklist for any new column/table:

1. Add to `migrations/0001_initial_schema.sql` **and** the idempotent block in
   `db/mod.rs` (existing DBs only run the latter).
2. No semicolons or apostrophes in comments (§2.5).
3. Add the field to the struct in `schema.rs` with `#[serde(default)]` if rows may
   predate it.
4. Add a repo method; do not hand-write SQL in a route.
5. Add a test covering the new behaviour.

---

## 7. Verification bar (run before saying "done")

```bash
# Rust — must be 0 failures AND 0 warnings
cargo build --workspace
cargo test --workspace

# Web — must be 0 errors, 0 warnings
cd src/apps/web && npm run check && npm run build

# Built-in plugins — separate crates, not workspace members
./compile.sh --plugins
```

If a check cannot be run, **say so explicitly** and explain why. Do not report
success you did not observe.

Known environment quirks:

- `~/.cargo`, `~/.npm`, `~/.gradle`, `pnpm` store and `rustup` may be read-only.
  Use `CARGO_HOME=/tmp/cargo-home`, `npm_config_cache=/tmp/npm-cache`,
  `GRADLE_USER_HOME=/tmp/gradle-home`.
- A fresh `DATABASE_URL` path fails with `unable to open database file` unless the
  parent directory exists and is writable — copy an existing DB instead.

---

## 8. Deploying a server

The server is one static binary plus `data/` and `plugins/`. It has **no CLI
flags**: `neuxdot-server --help` does not print help, it starts the server on the
default port. Configure it with environment variables only.

### 8.1 Build

```bash
./compile.sh --server     # release/neuxdot-server (musl static when available)
./compile.sh --plugins    # release/plugins/{computer,chat,gitlab,telegram,discord}
```

`--server` falls back to a glibc build when neither `musl-gcc` nor Docker is
available. Check with `file release/neuxdot-server` before copying to a host
with a different libc.

### 8.2 Host prerequisites

```bash
apt-get install -y bubblewrap socat
```

- **`bwrap`** — every stdio/binary plugin runs sandboxed unless its manifest
  declares `elevated`. Without it those plugins refuse to start.
- **`socat`** — required only for a network allow-list: the relay inside the
  sandbox forwards `127.0.0.1:41873` to the host-side CONNECT proxy.
- Unprivileged user namespaces must be enabled (`bwrap --unshare-all`).
- `data/` must be writable: SQLite, `data/sandbox/`, `data/media/`.

### 8.3 First deploy

```bash
rsync -a --info=progress2 \
  -e "ssh -J <jump-host>" \
  /path/to/dot/ root@<host>:/opt/neuxdot/
```

`/path/to/dot` is the runtime directory (binary, `plugins/`, `data/`), not the
source checkout. Create the target first: `ssh … 'mkdir -p /opt/neuxdot'`.

### 8.4 systemd unit

`/etc/systemd/system/neuxdot.service`:

```ini
[Unit]
Description=NeuxDOT server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=/opt/neuxdot
Environment=PORT=35950
Environment=RUST_LOG=info,neuxdot_core=debug,neuxdot_server=debug
ExecStart=/opt/neuxdot/neuxdot-server
Restart=always
RestartSec=3
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

```bash
systemctl daemon-reload
systemctl enable --now neuxdot
```

`PORT` is the only required setting. The seeded Server Control plugin is pinned
to `http://127.0.0.1:$PORT/api/control/mcp`, so changing the port is safe — the
next boot refreshes that URL.

### 8.5 Updating

```bash
./compile.sh --server
rsync -a release/neuxdot-server root@<host>:/opt/neuxdot/neuxdot-server
ssh root@<host> 'systemctl restart neuxdot'
```

Never rsync a stale local `data/` over a live host: it replaces the database and
every credential in it. Upload `data/` only on the first deploy or when you
intend to restore a specific backup.

### 8.6 Verify

```bash
systemctl is-active neuxdot
ss -ltnp | grep 35950
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:35950/api/auth/me   # 401
journalctl -u neuxdot -n 50 --no-pager
```

In the log, confirm:

- `NeuxDOT server listening on http://0.0.0.0:<PORT>`
- `spawning plugin on the host (elevated): ./data/binaries/computer`
- `sandbox proxy listening on … with N destination rule(s)` for allow-listed plugins

Then kill the process once (`systemctl kill -s SIGKILL neuxdot`) and confirm
systemd brings it back — `Restart=always` is part of the deployment, not a
nice-to-have.

### 8.7 Deployment rules

- `data/neuxdot.sqlite` contains bot tokens, provider keys and password hashes.
  It is git-ignored; never commit it, never upload it to a shared host, and
  treat `/opt/neuxdot/data` as sensitive.
- Do not run the server as root if you can avoid it. If you do, remember that
  `elevated` plugins inherit root. `bwrap` needs user namespaces, not root.
- `data/media/` must stay writable: `imageView` stores decoded attachments there.
- The server binds `0.0.0.0`. Put it behind a reverse proxy/TLS if it is
  reachable from the internet; the API is bearer-token authenticated but plain
  HTTP exposes those tokens on the wire.

---

## 9. Anti-patterns seen in this repo (do not repeat)

| Symptom | Real cause |
|---|---|
| `unsafe` block failing at startup | Apostrophe/semicolon in a SQL comment |
| `insufficient tool messages` | Compaction dropped sibling tool replies |
| Page 500 on a route | Handler existed for PUT/DELETE but not GET |
| UI shows stale plugin behaviour | Rebuilt `.wasm` never re-uploaded / iframe cached by id only |
| `FOREIGN KEY constraint failed` on storage | Name-space used a prefixed id; `extension_kv` FKs `extensions(id)` (unprefixed) |
| Sharee gets `403` on a shared bot | Ownership check left in place instead of capability check |
| Message labelled with wrong user | Author not recorded; sender must come from host stamp |

---

## 10. Before you open a PR

- [ ] No new compiler warnings; no `never used` items.
- [ ] No `unwrap()` on a request path.
- [ ] Authorization uses capability helpers, not `account_id ==`.
- [ ] New public behaviour has a test.
- [ ] Migration is idempotent and comment-safe.
- [ ] `cargo test --workspace` and `npm run check` both pass.
- [ ] Diff contains only the intended change.
