# Security

VectorCode is a **local-first** tool: the MCP server reads files from your
workspace and embeds them into a local SQLite database. There is no remote
service, no telemetry, and no multi-tenant backend. The threat model below
reflects that scope.

## Threat model

VectorCode processes three categories of input:

1. **Workspace files** — source code in the project the user initializes.
2. **Client requests** — MCP tool calls from the agent (e.g. `vec_read_lines`,
   `vec_outline`, `vec_search`).
3. **CLI arguments** — `vectorcode outline <path>`, `vectorcode index --file <path>`.

The primary trust boundary is between **workspace files** (trusted) and
**client/CLI-supplied paths** (semi-trusted). An agent calling `vec_read_lines`
with a crafted `file_path` must not be able to read files outside the
initialized workspace; a CLI user passing `index --file ../../etc/passwd`
must not get that file indexed.

The adversary model is a **prompt-injected or careless agent**, or a
**misconfigured automation script**. We do not defend against a local user
with shell access — if you can run `cat /etc/passwd` you do not need
VectorCode to leak it.

## Validated defenses (phase-4.2)

The following defenses are in place and covered by regression tests
in `tests/security_audit_*.rs`:

| ID | Defense | Test |
|----|---------|------|
| R3 | Indexer skips files whose canonical path falls outside the project root. Symlinks pointing outside the workspace are silently skipped with a `tracing::warn!`. | `security_audit_indexer` |
| R5 | All path validation goes through `src/mcp/security.rs::resolve_within_workspace` and `resolve_within_project`. No duplicated canonicalize+starts_with blocks. | `security_audit_mcp` |
| R7 | `AppState.workspaces` is a `BTreeMap`, so workspace iteration order is deterministic. When two workspaces both own a file, the lexicographically first root wins consistently. | `security_audit_mcp::resolve_within_workspace_is_deterministic_across_overlaps` |
| R5 | Graph tools (`vec_find_callers`, `vec_find_dependents`, `vec_trace_imports`) validate the optional `file_path` and silently return empty results for invalid disambiguation paths. | `security_audit_mcp` |
| R10 | `vectorcode outline <path>` rejects paths outside the project root with a non-zero exit and a clear stderr message. | `security_audit_cli::cli_outline_rejects_*` |
| R11 | `vectorcode index --file <path>` rejects paths outside the project root. | `security_audit_cli::cli_index_file_rejects_*` |
| R5 | Watcher filters canonicalize both the file path and the project root before `starts_with`, so alias forms (e.g. `../repo`) are recognized. | `src/watcher/gitignore.rs`, `src/watcher/mod.rs` |
| REQ-SEC-06 | No `.unwrap()` or `.expect(` in library code, enforced by a runtime scan of `src/**/*.rs`. | `security_audit_config` |

## Known limits (deferred)

The following items were identified during the audit but **explicitly
deferred** to future phases. They are not covered by the current
defenses — see the design rationale before relying on VectorCode in
hostile environments.

### R1 — No root allowlist

There is no `--allow-root` flag or allowlist config. The MCP server
trusts whatever roots the client advertises via the MCP `roots/list`
protocol. A prompt-injected agent could advertise a sensitive directory
(like `/`) as a root and then read files from it via `vec_read_lines`.

**Mitigation today**: only initialize VectorCode in directories you
control. Do not point the client at sensitive system roots.

### R2 — No gitignore/exclude gate on read tools

`vec_read_lines` and `vec_outline` will happily return the contents of
files matched by `.gitignore` or `vectorcode.exclude_extensions`. Only
`index` respects the exclusion list.

**Mitigation today**: put secrets in `.gitignore` (which VectorCode
respects during indexing) so they never enter the vector DB. Note that
*if you add a file to `.gitignore` after indexing*, the file is still in
the DB — re-run `vectorcode index --full` to rebuild.

### R4 — TOCTOU gap in `read_lines`/`outline`

The MCP handlers canonicalize the path, check the boundary, and then
call `tokio::fs::read_to_string`. A TOCTOU attacker could swap the
target file between the canonicalize and the read. Realistic risk is
low (the agent already needs local write access to win the race), but
the canonical fix is `File::open + take(2MB)` to bound the read.

### R6 — `vec_reindex full=true` has no confirmation

`vec_reindex` with `full=true` drops the entire index. There is no
confirmation prompt or undo. A prompt-injected agent could trigger this
silently.

### R8 — No rate limiting

There is no rate limit on MCP tool calls. A prompt-injected agent
could issue thousands of `vec_search` calls in a loop, burning CPU
and embedding API quota.

## Operational recommendations

1. **Initialize VectorCode in a dedicated project directory**, not in
   `$HOME` or `/`. The narrower the workspace root, the smaller the
   blast radius if a client supplies a malicious path.

2. **Use a non-prod embedder for agent experiments.** Gemini and OpenAI
   keys in `.vectorcode/config.toml` will be billed for every
   `vec_search` call. Consider a local ONNX or Ollama embedder for
   agent-facing deployments.

3. **Review the `.gitignore` and `exclude_*` lists** in
   `.vectorcode/config.toml` before indexing. Anything in these lists
   will not enter the vector DB.

4. **Treat the vector DB as sensitive.** `vec_search` returns semantic
   matches, which can leak file content to an agent that knows what to
   ask for. The DB file is at `.vectorcode/index.db` and is
   gitignored by default.

5. **Do not point the MCP client at system roots.** If the client
   advertises `/` or `$HOME` as a root, the `vec_read_lines` and
   `vec_outline` defenses only protect files inside *initialized*
   workspaces (those with a `.vectorcode/` directory).

## Reporting issues

Security issues can be reported via GitHub Issues. For sensitive
disclosures, open a private security advisory on the GitHub repository.
