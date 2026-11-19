# avrig-api
Unified cross-platform `avrig` executable and backend API runtime for avrig-ai.

## Build and test

```powershell
cargo test
cargo build --release
```

```sh
cargo test
cargo build --release
```

Both platforms build the same `avrig` target, with the host platform adding its
normal executable suffix.

## Run

Configuration defaults to `config/` next to the executable. TOML files in that
directory are merged alphabetically. An explicit file or directory can be
selected with `--config`:

```text
avrig --help
avrig --config ./config run
avrig --config ./config serve
avrig --config ./config watch
```

From the repository root, `make run` builds the Linux release executable and starts
the API server in the background inside the `avrig-ai` WSL environment. Its PID
and stdout/stderr logs are stored in `.run/`. `make stop` stops the WSL instance recorded
there. To run natively on Windows, use `make run-windows` and `make stop-windows`.

On Windows, `make deploy` builds the release executable and copies only
`avrig.exe` to `D:\avrig`, preserving the deployment's existing config and logs.
It deploys and registers the executable as an automatic Windows Service (`avrig-watcher`)
using NSSM, restarting the service cleanly after copying. It also deploys the Linux
executable to `/opt/avrig/avrig` inside the `avrig-ai` WSL instance.

Both targets carry their answer model through `<exec dir>/config/server.toml`, and
the deploy keeps that file per node rather than copying the repository template,
which binds `0.0.0.0`. On Linux it rewrites only the `answer_model` line of
`/opt/avrig/config/server.toml` and leaves the loopback host, port, upload
directory, and search threshold untouched; a missing file fails the deploy instead
of being created blind. On Windows it creates `D:\avrig\config\server.toml` once
with an explicit `host = "127.0.0.1"`, then patches only `answer_model` on later
runs. The previous file is saved beside it before any change, and the deploy ends
by printing the effective `host` and `answer_model` for each target.

Application logs use the shared `avrig-logging` crate and are written as
newline-delimited JSON to `logs/avrig.log` relative to the process working
directory. Records include UTC RFC 3339 timestamps, level, tracing target,
source file, line number, message, and structured fields. The active log rotates
at 10 MiB by default, retaining five numbered archives (`avrig.1.log`, etc.).
`[logging]` settings in `config.toml` can change the level, output directory,
size limit in MiB, and archive count.

`run` uses the roles in `config/config.toml`. The initial default role is the
HTTP server. The API exposes `/api/v1/health`,
`HEAD|GET /api/v1/documents/by-hash/{sha256}`, `POST /api/v1/documents/upload`,
`GET|POST /api/v1/knowledge/search`, and source-reference reads. Search and the
other document routes require an `Authorization: Bearer <access-token>` header.
They validate the JWT signature against Rauthy's discovered JWKS, and check
issuer, audience, and expiry. A service token must also carry every scope listed
in `oidc.required_scopes` (`documents:read` by default); an interactive token
carrying the `openid` scope is accepted without that check.

## OpenCode Big Pickle

`POST /v1/chat/completions` and `POST /api/v1/chat/completions` forward
OpenAI-compatible chat requests to OpenCode Console using the `big-pickle`
model. The backend overrides the caller's `model` with its configured model and
passes the Console response through, including streamed responses. Configure a
Console API key in the `OPENCODE_CONSOLE_API_KEY` environment variable.
Configuration is in `config/opencode.toml`.

The JEV intent-classification endpoint remains available at
`POST /api/v1/jev/chat/completions`. OpenCode documents that external clients
receive HTTP 403 for free-tier chat models; direct Console access to Big Pickle
therefore requires an account plan and model access that permits external
clients.

```json
{
  "model": "big-pickle",
  "messages": [{"role": "user", "content": "Say hello in one sentence."}]
}
```

## Knowledge search

`GET /api/v1/knowledge/search` and `POST /api/v1/knowledge/search` are the single
search endpoint. Both run the same pipeline: they embed the query, pull a
candidate set from `km.document_vector`, rerank the passages, and return them
with provenance. The only difference between the two methods is how the request
parameters arrive. Neither method classifies intent.

`POST` takes a JSON body. `GET` takes the same fields as query parameters, with
`q` or `query` for the question:

```json
{"query": "schema migration", "limit": 5, "threshold": -2.0}
```

```text
GET /api/v1/knowledge/search?q=schema+migration&limit=5
```

`limit` defaults to `5` and must be between `1` and `50`. `query` is trimmed and
must be 1 to 4096 characters. `threshold` is optional and accepts any finite
`f32`.

### Choosing a transport

Both methods accept a `stream` option: a boolean body field on `POST`, the query
parameter `stream` on `GET`. The value resolves in this order:

| `stream` | Response |
| :--- | :--- |
| `true` | `text/event-stream` |
| `false` | JSON, even when the client sends an event-stream `Accept` header |
| absent | negotiate on `Accept`; SSE when it contains `text/event-stream`, JSON otherwise |

An explicit `stream` always wins over the `Accept` header.

Without streaming, the response carries the retrieved passages only:

```json
{
  "query": "schema migration",
  "count": 2,
  "reranked": true,
  "results": [
    {
      "chunk_id": "uuid",
      "document_id": "uuid",
      "document_code": "DOC-1A2B3C4D",
      "document_name": "Database operations",
      "file_name": "database-ops.md",
      "mime_type": "text/markdown",
      "sha256": "hex",
      "chunk_index": 3,
      "page_no": 2,
      "chunk_text": "Migrations are applied with sqlx.",
      "vector_score": 0.81,
      "rerank_score": -1.25,
      "score": -1.25
    }
  ]
}
```

`vector_score` is cosine similarity from pgvector, in `-1.0..=1.0`.
`rerank_score` is the raw cross-encoder logit from `bge-reranker-v2-m3`, not a
probability: it is unbounded, negative values are normal even for the best
passage, the useful spread is roughly `-11.0..=8.0`, and a threshold of `0.0`
corresponds to sigmoid probability `0.5`. `score` is whichever value produced
the returned order.

`threshold` filters after ranking and keeps passages with `score >= threshold`.
It is applied only when reranking succeeded, because vector similarity and
reranker logits are different scales. When the reranker is not configured, times
out, or fails, the response keeps vector order, sets `reranked` to `false` and
`rerank_score` to `null`, logs a warning without query text, and ignores
`threshold`.

With streaming, the response is a sequence of server-sent events:
`metadata` (query, count, reranked), then one or more `answer_delta` events
carrying the synthesized answer, then `citations` holding the same passages in a
`results` field, then `done`. When no answer can be generated the stream still
completes: it emits `metadata`, a single fallback `answer_delta`, `citations`,
and `done`. A failure in the middle of generation emits an `error` event before
the stream continues to `citations`.

### Answer model

Streaming answers are generated from the retrieved passages by the model named in
`server.answer_model`, which defaults to `opencode/big-pickle`. The value is a
`provider/model` string, so `opencode` selects the gateway's `opencode` provider
and `big-pickle` is forwarded upstream unchanged. That provider authenticates
with `OPENCODE_API_KEY`, which is separate from the `OPENCODE_CONSOLE_API_KEY`
used by the console client described above. When the key is missing or the
provider is not configured, search itself still succeeds and the answer falls
back as described.

Validation errors, including a blank query, a query over 4096 characters, a
`limit` outside `1..=50`, and a non-finite `threshold`, return `400`. A missing
or invalid bearer token returns `401` when OIDC is configured.

### Removed search paths

The earlier search routes are gone and answer `410 Gone` with a JSON `error`
field naming the new endpoint. They are kept as tombstones so an old client gets
an explicit answer instead of the console's `index.html` fallback:

```text
/api/v1/documents/search
/api/v1/documents/search/stream
/v1/documents/search
/v1/documents/search/stream
/v1/search
```

Document management is unchanged: `POST /api/v1/documents/upload`,
`HEAD|GET /api/v1/documents/by-hash/{sha256}`, and
`GET /api/v1/documents/{id}/sources`.

### Reranker settings

Reranker settings live in `config/reranker.toml` under `[reranker]`. `base_url`
is required for reranking and the client appends `/reranker`. `request_timeout_ms`
defaults to `60000`. `default_top_k` serves callers that omit a limit, and
`max_candidates` bounds the candidate set. `ca_cert_path` optionally adds a PEM
bundle of extra root CAs; when absent the platform trust store is used and
certificate verification is always on.

The watcher monitors configured folders, applies include/exclude filters,
debounces changes, and calculates streaming SHA-256 hashes only for a built-in
allowlist of document formats (Markdown, HTML, PDF, Word, Excel, PowerPoint,
OpenDocument, text, CSV/TSV, reStructuredText, AsciiDoc, Org, TeX, and EPUB).
JSON, images, and other non-document files are ignored. Remote preflight and
upload are not enabled until the authenticated upload client is implemented.
No database or physical EDM objects are deployed by this binary.

See `config/` for starter TOML files. Do not put plaintext credentials or Vault
tokens in configuration files.
