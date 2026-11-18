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
`POST /api/v1/documents/search`, and source-reference reads. Document routes
require an `Authorization: Bearer <access-token>` header. They validate the JWT
signature against Rauthy's discovered JWKS, and check issuer, audience, expiry,
and the configured scope (`documents:read` for search and hash lookup).

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

## Document search

`POST /api/v1/documents/search` is retrieval only: it embeds the query, pulls a
candidate set from `km.document_vector`, reranks the passages, and returns them
with provenance. It does not classify intent, generate an answer, or invoke an
agent.

```json
{"query": "schema migration", "limit": 5, "threshold": -2.0}
```

`limit` defaults to `5` and must be between `1` and `50`. `query` is trimmed and
must be 1 to 4096 characters. `threshold` is optional and accepts any finite
`f32`.

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
