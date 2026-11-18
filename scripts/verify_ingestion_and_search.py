import urllib.request, urllib.parse, ssl, json, os

ctx = ssl.create_default_context()
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE

# 1. Acquire Token
token_url = "https://iam.avrig.ai/auth/v1/oidc/token"
data = urllib.parse.urlencode({
    "grant_type": "client_credentials",
    "client_id": "avrig-watcher",
    "client_secret": "aIVdKmmeElQrQXQFuywUUpUHhILdvRIlswiwCheHzKepGKuJvGaEFcnzlaxmSIYg",
    "scope": "openid documents:read documents:write"
}).encode('utf-8')

req = urllib.request.Request(token_url, data=data, headers={"Content-Type": "application/x-www-form-urlencoded"})
with urllib.request.urlopen(req, context=ctx) as resp:
    token_json = json.loads(resp.read().decode('utf-8'))

token = token_json["access_token"]
print("Token acquired successfully! Length:", len(token))

# 2. Upload sample fixtures
fixtures_dir = r"/mnt/d/Cloud/OneDrive/AI/Agentic/avrig-api/tests/fixtures" if os.path.exists("/mnt/d") else r"D:\Cloud\OneDrive\AI\Agentic\avrig-api\tests\fixtures"
import glob
files = glob.glob(os.path.join(fixtures_dir, "*"))

for fpath in files:
    fname = os.path.basename(fpath)
    with open(fpath, "rb") as f:
        file_bytes = f.read()

    boundary = "----WebKitFormBoundary7MA4YWxkTrZu0gW"
    body = bytearray()
    body.extend(f"--{boundary}\r\n".encode('utf-8'))
    body.extend(f'Content-Disposition: form-data; name="file"; filename="{fname}"\r\n'.encode('utf-8'))
    body.extend(b"Content-Type: application/octet-stream\r\n\r\n")
    body.extend(file_bytes)
    body.extend(f"\r\n--{boundary}--\r\n".encode('utf-8'))

    upload_req = urllib.request.Request(
        "https://api.avrig.ai/api/v1/documents/upload",
        data=body,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": f"multipart/form-data; boundary={boundary}"
        }
    )

    try:
        with urllib.request.urlopen(upload_req, context=ctx) as resp:
            res = json.loads(resp.read().decode('utf-8'))
            print(f"Uploaded {fname}: {res}")
    except Exception as e:
        print(f"Error uploading {fname}: {e}")

# 3. Test Search queries
queries = [
    "cryptographic signed lifecycle and stage gates",
    "PostgreSQL replica failover consensus",
    "dense neural vector representations with HNSW indexing",
    "subagents governed by cryptographic consensus"
]

for q in queries:
    search_payload = json.dumps({"query": q, "limit": 2}).encode('utf-8')
    search_req = urllib.request.Request(
        "https://api.avrig.ai/api/v1/documents/search",
        data=search_payload,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json"
        }
    )
    with urllib.request.urlopen(search_req, context=ctx) as resp:
        search_res = json.loads(resp.read().decode('utf-8'))
        mode = "reranked" if search_res.get("reranked") else "vector order"
        print(f"\nSearch '{q}': found {search_res['count']} matches ({mode})")
        for match in search_res["results"]:
            rerank = match.get("rerank_score")
            label = f"rerank={rerank:.4f}" if rerank is not None else "rerank=n/a"
            print(
                f"  - [{match['document_code']} {match['file_name']}] "
                f"(vector={match['vector_score']:.4f} {label}): {match['chunk_text'][:90]}..."
            )
