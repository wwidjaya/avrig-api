#!/bin/bash
set -e

# CLIENT_SECRET is the OAuth client secret for the service client. It is
# deliberately absent from this file. Export it before running the script,
# sourcing the value from a local untracked environment file (for example
# .secrets/ingest.env) or a secret manager.
: "${CLIENT_SECRET:?CLIENT_SECRET must be set in the environment}"

# CLIENT_ID is a non-secret identifier and keeps its previous default. Override
# it in the environment if you need to test a different service client.
CLIENT_ID="${CLIENT_ID:-avrig-watcher}"

RESP=$(curl -s -k -X POST https://iam.avrig.ai/auth/v1/oidc/token \
  -H "Content-Type: application/x-www-form-urlencoded" \
  --data-urlencode "grant_type=client_credentials" \
  --data-urlencode "client_id=$CLIENT_ID" \
  --data-urlencode "client_secret=$CLIENT_SECRET" \
  --data-urlencode "scope=documents:read documents:write")

TOKEN=$(echo "$RESP" | jq -r '.access_token // empty')

if [ -z "$TOKEN" ]; then
  echo "Failed to get token: $RESP"
  exit 1
fi

echo "Successfully acquired token. Length: ${#TOKEN}"

for filepath in /mnt/d/Cloud/OneDrive/AI/Agentic/avrig-api/tests/fixtures/*; do
  fname=$(basename "$filepath")
  echo "--- Uploading $fname ---"
  UPLOAD_RESP=$(curl -s -k -X POST https://api.avrig.ai/api/v1/documents/upload \
    -H "Authorization: Bearer $TOKEN" \
    -F "file=@$filepath")
  echo "$UPLOAD_RESP"
done

echo "--- Searching 'cryptographic signed lifecycle' ---"
curl -s -k -X POST https://api.avrig.ai/api/v1/knowledge/search \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "cryptographic signed lifecycle", "limit": 3}' | jq .

echo "--- Searching 'PostgreSQL replica failover consensus' ---"
curl -s -k -X POST https://api.avrig.ai/api/v1/knowledge/search \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "PostgreSQL replica failover consensus", "limit": 3}' | jq .

echo "--- Searching 'dense neural vector representations' (PDF fixture) ---"
curl -s -k -X POST https://api.avrig.ai/api/v1/knowledge/search \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "dense neural vector representations", "limit": 3}' | jq .

echo "--- Searching 'subagents governed by cryptographic consensus' (DOCX fixture) ---"
curl -s -k -X POST https://api.avrig.ai/api/v1/knowledge/search \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "subagents governed by cryptographic consensus", "limit": 3}' | jq .
