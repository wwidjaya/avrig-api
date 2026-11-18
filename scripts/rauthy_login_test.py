import urllib.request, ssl, re, json, hashlib

ctx = ssl.create_default_context()
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE

opener = urllib.request.build_opener(urllib.request.HTTPSHandler(context=ctx), urllib.request.HTTPCookieProcessor())

# 1. Authorize GET to fetch CSRF and session cookies
auth_url = "https://iam.avrig.ai/auth/v1/oidc/authorize?client_id=rauthy&response_type=code&redirect_uri=https://iam.avrig.ai/auth/v1/oidc/callback&scope=openid&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256"
req = urllib.request.Request(auth_url)
html = opener.open(req).read().decode('utf-8')

csrf = re.search(r'id="tpl_csrf_token">([^<]+)<', html).group(1)
print("Fetched CSRF:", csrf)

# 2. Fetch POW challenge
pow_resp = json.loads(opener.open("https://iam.avrig.ai/auth/v1/pow").read().decode('utf-8'))
print("POW challenge:", pow_resp)
