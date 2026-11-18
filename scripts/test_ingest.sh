#!/bin/bash
set -e

CLIENT_SECRET="B4zDq_7W-0z1GgX4Gg9F7l3P3m4Q8w9R"

RESP=$(curl -s -k -X POST https://iam.avrig.ai/auth/v1/oidc/token \
  -H "Content-Type: application/x-www-form-urlencoded" \
  --data-urlencode "grant_type=client_credentials" \
  --data-urlencode "client_id=avrig-watcher" \
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
curl -s -k -X POST https://api.avrig.ai/api/v1/documents/search \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "cryptographic signed lifecycle", "limit": 3}' | jq .

echo "--- Searching 'PostgreSQL replica failover consensus' ---"
curl -s -k -X POST https://api.avrig.ai/api/v1/documents/search \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "PostgreSQL replica failover consensus", "limit": 3}' | jq .

echo "--- Searching 'dense neural vector representations' (PDF fixture) ---"
curl -s -k -X POST https://api.avrig.ai/api/v1/documents/search \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "dense neural vector representations", "limit": 3}' | jq .

echo "--- Searching 'subagents governed by cryptographic consensus' (DOCX fixture) ---"
curl -s -k -X POST https://api.avrig.ai/api/v1/documents/search \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "subagents governed by cryptographic consensus", "limit": 3}' | jq .
