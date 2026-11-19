# Authentication and sessions - Slack web-client API

## Summary

- Covers how a third-party client obtains and keeps a Slack **web-client session**: an `xoxc-` token (sent as the `token` form/JSON field) plus the `d` cookie (value `xoxd-...`, HttpOnly), and how that session dies (`invalid_auth`, `token_revoked`, ...).
- Public/private split: `auth.test`, `auth.revoke`, `auth.teams.list` are documented public methods. Everything sign-in related (`auth.signin`, `auth.findTeam`, `auth.loginMagic`, `signin.findWorkspaces`, `signup.*`, the `ssb/redirect` page, the `app.slack.com/auth` exchange) is private/undocumented.
- A signed-in browser session can mint per-workspace `xoxc` tokens on demand via `app.slack.com/auth` or the `ssb/redirect` HTML page; both are the key recovery paths when you hold a `d` cookie but no token.
- The email login flow is now an email-code + magic-token chain (`signup.checkEmail` -> `auth.captcha` -> `signup.confirmEmail` -> `signin.confirmCode` -> `signin.findWorkspaces` -> `auth.loginMagic`/`auth.loginMagicBulk`), not a password POST; password login (`auth.signin`) survives in old clients only.
- Biggest risks: CAPTCHA (`auth.captcha` sitekey) gating the email flow; Slack rejecting stale browser fingerprints/UA on the login page; `xoxc` no longer present in served HTML (localStorage/LevelDB is the source of truth); per-workspace token selection inside Enterprise Grid orgs.

## Endpoints

### auth.test

- Transport: `POST https://slack.com/api/auth.test` (also `POST https://<ws>.slack.com/api/auth.test`), form-encoded (`application/x-www-form-urlencoded`) or JSON. Token via `Authorization: Bearer` header or `token` POST param.
- Visibility: public.
- Request params:
  - `token` - string - required - the session token to validate (xoxc/xoxs/xoxp/xoxb).
- Response (short excerpt):

```json
{
  "ok": true,
  "url": "https://subarachnoid.slack.com/",
  "team": "Subarachnoid Workspace",
  "user": "grace",
  "team_id": "T12345678",
  "user_id": "W12345678"
}
```

- For Enterprise Grid org-level auth, `enterprise_id` is included; bot tokens add `bot_id`. Failure: `{"ok":false,"error":"invalid_auth"}`; no token: `not_authed`.
- Pagination: none.
- Used for: validating stored credentials at startup / after fetch; canonical "who am I on which workspace" call. slackdump runs it on every provider to build its session cache (`AuthTestContext`); mautrix keys its per-(team,user) login on a boot call instead (see `client.userBoot`, one line below).
- OBSERVED: https://docs.slack.dev/reference/methods/auth.test
- OBSERVED: rusq/slackdump/auth/auth.go:128-145 (`Test` -> `cl.AuthTestContext`)
- OBSERVED: rusq/slackdump/internal/edge/wrapper.go:44-45 (`AuthTestContext` passthrough)
- Cross-area, one line: the web client itself validates the session with `client.userBoot` rather than `auth.test`; mautrix calls `ClientUserBootContext` immediately after token login — OBSERVED: mautrix/slack/pkg/connector/client.go:309.

### auth.revoke

- Transport: `POST https://slack.com/api/auth.revoke`, form-encoded, `token` field.
- Visibility: public.
- Request params:
  - `token` - string - required - token to revoke (defaults to the caller's own).
- Response:

```json
{ "ok": true, "revoked": true }
```

- Pagination: none.
- Used for: "sign out and kill this token" (e.g. a per-workspace logout button).
- OBSERVED: beeper/slackgo/auth.go:9,33-45 (`SendAuthRevokeContext` -> `auth.revoke`)

### auth.teams.list

- Transport: `POST https://slack.com/api/auth.teams.list`, form-encoded.
- Visibility: public.
- Request params:
  - `token` - string - required
  - `cursor` - string - optional - pagination cursor from `response_metadata.next_cursor`
  - `limit` - int - optional - page size
  - `include_icon` - bool - optional - include team icon objects
- Response (short excerpt):

```json
{
  "ok": true,
  "teams": [
    {
      "id": "T12345678",
      "name": "Subarachnoid Workspace",
      "domain": "subarachnoid"
    }
  ],
  "response_metadata": { "next_cursor": "..." }
}
```

- Pagination: cursor-based (`cursor` / `response_metadata.next_cursor`).
- Used for: enumerating all workspaces a token can access; a sanity check that a session still spans the expected workspaces.
- OBSERVED: beeper/slackgo/auth.go:88-107 (`ListTeamsContext` -> `auth.teams.list`, `include_icon`, cursor handling)

### auth.signout

- Transport: `POST https://<ws>.slack.com/api/auth.signout`, form-encoded, `token` field.
- Visibility: private (not in docs.slack.dev method list; exists in the web-client method set).
- Request params:
  - `token` - string - required
- Response: `{"ok":true}` (generic SlackResponse).
- Pagination: none.
- Used for: signing out one workspace session without revoking other tokens.
- OBSERVED: beeper/slackgo/auth.go:54-67 (`SendAuthSignoutContext` -> `auth.signout`)
- OBSERVED: 3kh0/slack-datamine/build/api-methods.txt (in the web-client method list)

### auth.findTeam

- Transport: `POST https://slack.com/api/auth.findTeam`, multipart/form-data (slack-libpurple builds a multipart body with `Content-Type: multipart/form-data`, slack-api.c:138-181; Sup posts `FormData`, src/utils/http/index.ts:88-94).
- Visibility: private.
- Request params:
  - `domain` - string - required - workspace subdomain (the `<ws>` in `<ws>.slack.com`)
- Response:

```json
{ "ok": true, "team_id": "T024BE7LD" }
```

- Pagination: none.
- Used for: first step of email/password sign-in: resolve the subdomain the user typed to a `team_id` for `auth.signin`.
- OBSERVED: arnnis/Sup/src/slices/team-thunks.ts:44-50 (`http({path:'/auth.findTeam', body:{domain}})` -> `team_id`)
- OBSERVED: dylex/slack-libpurple/slack-auth.c:96-104 (`slack_api_post(..., "auth.findTeam", "domain", sa->host, ...)` -> `team_id`)
- OBSERVED: 3kh0/slack-datamine/build/api-methods.txt (listed)

### auth.findUser

- Transport: `POST https://<ws>.slack.com/api/auth.findUser`, multipart/form-data (slack-libpurple slack-api.c:138-181).
- Visibility: private.
- Request params:
  - `email` - string - required
  - `team` - string - required - `team_id` from `auth.findTeam`
- Response:

```json
{ "ok": true, "user_id": "U12345678" }
```

- Pagination: none.
- Used for: mapping the account email to a `user_id` before password sign-in.
- OBSERVED: dylex/slack-libpurple/slack-auth.c:70-93 (`"auth.findUser", "email", ..., "team", ...` -> `user_id`)

### auth.signin

- Transport: `POST https://<ws>.slack.com/api/auth.signin`, multipart/form-data (slack-libpurple slack-api.c:138-181; Sup `FormData`, src/utils/http/index.ts:88-94).
- Visibility: private.
- Request params:
  - `email` - string - required (older clients send `user`=user_id; Sup sends `email`)
  - `password` - string - required
  - `team` - string - required - team_id
  - `pin` - string - optional - 2FA code; when 2FA is required the first call fails with `missing_pin` and the code is sent in `pin`
- Response:

```json
{
  "ok": true,
  "token": "xoxc-...",
  "user": "U12345678",
  "user_email": "grace@example.com"
}
```

- Errors observed: `team_not_found`, `missing_pin`, `user_not_found`, `incorrect_password`.
- Pagination: none.
- Used for: legacy email+password sign-in. No actively-maintained client ships it anymore: mautrix-slack's 2026 flow uses the email-code chain (see below), slackdump drives the browser login page, wee-slack and emacs-slack take token+cookie directly.
- OBSERVED: arnnis/Sup/src/slices/team-thunks.ts:52-64 (`auth.signin` with `{email, password, team, pin}` -> `{token, user, user_email}`)
- OBSERVED: arnnis/Sup/src/slices/team-thunks.ts:72-77 (error strings incl. `incorrect_password`)
- OBSERVED: dylex/slack-libpurple/slack-auth.c:33-50 (`"auth.signin", "user", ..., "password", ..., "team"` -> `token`)

### signup.checkEmail

- Transport: `POST https://slack.com/api/signup.checkEmail`, multipart/form-data, `_x_id` query, `Referer: https://slack.com/signin` (mautrix postAPI, login-email-api.go:577-608).
- Visibility: private.
- Request params:
  - `email` - string - required
  - `get_info` - string - sent as `"undefined"` by the web client
- Response:

```json
{ "ok": true, "challenge_response": true }
```

- `challenge_response: true` means Slack demands a CAPTCHA before emailing a login code.
- Pagination: none.
- Used for: first step of the current (2026) email sign-in chain; decides whether `auth.captcha` is needed.
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:100-128 (`RequestCode` -> `signup.checkEmail`, `challenge_response`)

### auth.captcha

- Transport: `POST https://slack.com/api/auth.captcha`, multipart/form-data, `_x_reason=fetch-auth-captcha-signin` form field (mautrix postAPI, login-email-api.go:577-608).
- Visibility: private.
- Request params:
  - (none observed; called after a `challenge_response` from `signup.checkEmail`)
- Response:

```json
{ "ok": true, "sitekey": "..." }
```

- Pagination: none.
- Used for: fetching the CAPTCHA sitekey to render in a webview when Slack requires a challenge for email sign-in. A native client cannot skip this; it must render the challenge (the web client renders it under the sitekey) and submit the solution as `captcha_response` to `signup.confirmEmail`.
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:115-126 (`auth.captcha` with reason `fetch-auth-captcha-signin` -> `sitekey`)

### signup.confirmEmail

- Transport: `POST https://slack.com/api/signup.confirmEmail`, multipart/form-data, `_x_id` query, `Referer: https://slack.com/signin` (mautrix postAPI, login-email-api.go:577-608).
- Visibility: private.
- Request params:
  - `email` - string - required
  - `locale` - string - observed `"en-US"`
  - `entry_point` - string - observed `"signin"`
  - `captcha_response` - string - required when `auth.captcha` returned a sitekey
- Response: `{"ok":true}` — Slack then emails a 6-character confirmation code (format `xxx-xxx`, hyphen optional on input).
- Errors observed around this step: `captcha_required`, `captcha_failed`, `invalid_captcha`, `ratelimited`, `rate_limited`, `code_request_limit_reached`, `invalid_email`, `email_invalid`.
- Pagination: none.
- Used for: requesting the email login code (the passwordless sign-in entry point).
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:137-150 (`confirmEmail` -> `signup.confirmEmail`, params and `captcha_response`)
- OBSERVED: mautrix/slack/pkg/connector/login-email.go:155-157 (`code_request_limit_reached`), :178-182 (`invalid_code`, `expired_code` handling in the next step)

### signin.confirmCode

- Transport: `POST https://slack.com/api/signin.confirmCode`, multipart/form-data, `_x_id` query (mautrix postAPI, login-email-api.go:577-608).
- Visibility: private.
- Request params:
  - `email` - string - required
  - `code` - string - required - 6-char code from the email (`^[a-zA-Z0-9]{3}-?[a-zA-Z0-9]{3}$`, hyphen stripped before sending)
- Response:

```json
{
  "ok": true,
  "magic_login_url": "https://.../magic-login/...",
  "has_workspace": true
}
```

- Errors observed: `invalid_code`, `invalid_email_code`, `code_already_used`, `failed`, `expired_code`, `code_expired`, `ratelimited`.
- Pagination: none.
- Used for: exchanging the emailed code for per-workspace magic login material (then `signin.findWorkspaces` lists what you can open).
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:155-165 (`signin.confirmCode` -> `magic_login_url`, `has_workspace`)
- OBSERVED: mautrix/slack/pkg/connector/login-email.go:55 (code regex), :402 (same regex as step-validation pattern), :176 (normalize: `strings.ReplaceAll(code, "-", "")`)
- OBSERVED: mautrix/slack/pkg/connector/login-email.go:179-183 (error strings)

### signin.findWorkspaces

- Transport: `POST https://slack.com/api/signin.findWorkspaces`, multipart/form-data, `_x_reason=get_started_workspaces` (mautrix postAPI, login-email-api.go:577-608).
- Visibility: private.
- Request params:
  - (none observed; follows a confirmed email code in the same cookie session)
- Response (shape from mautrix structs):

```json
{
  "ok": true,
  "confirmed_email": "grace@example.com",
  "current_teams": [
    {
      "email": "grace@example.com",
      "teams": [
        {
          "id": "T12345678",
          "name": "Subarachnoid",
          "url": "https://subarachnoid.slack.com/",
          "domain": "subarachnoid.slack.com",
          "magic_login_url": "https://...",
          "magic_login_code": "...",
          "sso_required": false,
          "two_factor_required": false
        }
      ]
    }
  ],
  "current_orgs": [
    {
      "email": "grace@example.com",
      "org": { "id": "E...", "...": "..." },
      "teams": []
    }
  ]
}
```

- Pagination: none.
- Used for: listing every workspace/org the confirmed email can sign into, including which ones need SSO (`sso_required`) or 2FA (`two_factor_required`) — this is the workspace-picker data source.
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:167-232 (`signin.findWorkspaces`, `slackFindWorkspacesResponse`, workspace fields incl. `magic_login_code`:198, `sso_required`:199, `two_factor_required`:200)

### auth.loginMagic

- Transport: `POST https://slack.com/api/auth.loginMagic`, multipart/form-data, `_x_id` query (mautrix postAPI, login-email-api.go:577-608).
- Visibility: private.
- Request params:
  - `magic_token` - string - required - the `magic_login_code` from `signin.findWorkspaces` (or parsed from `magic_login_url`)
  - `two_factor_native_supported` - string - observed `"1"`
  - `two_factor_is_backup` - string - observed `"0"`
  - `two_factor_pin` - string - required when the first call fails with a `missing_pin*` error (6-digit TOTP/SMS)
- Response:

```json
{ "ok": true, "token": "..." }
```

- (Token prefix left generic: no cited source checks it — see LEADS. mautrix treats the returned token as an auth token and separately harvests the `d` cookie from the response; the web client uses the same call on its magic-login pages.)
- Errors observed: `two_factor_required`, `missing_pin`, `missing_pin_app`, `missing_pin_app_sms`, `missing_pin_sms`, `missing_pin_sms_app`, `missing_pin_sms_sms` (all retried with `two_factor_pin`).
- Pagination: none.
- Used for: passwordless per-workspace sign-in; the code is single-use and short-lived.
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:375-393 (`nativeMagicLogin`: `magic_token`, `two_factor_*` fields -> `token`)
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:395-412 (`slackMagicLoginNeedsTwoFactorCode` error set)
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:331-373 (`StartTwoFactor`/`SubmitTwoFactor` orchestration)

### auth.loginMagicBulk

- Transport: `GET https://app.slack.com/api/auth.loginMagicBulk?magic_tokens=<code>&ssb=1` — GET with query params, JSON response; requires a cookie jar (the call itself sets the `d` cookie).
- Visibility: private.
- Request params:
  - `magic_tokens` - string - required - magic login code (the `magic-login` URL payload)
  - `ssb` - string - observed `"1"` (semantics not defined in any cited source; mautrix pairs it with the ssb-redirect "Use Slack in Browser" page flow)
- Response:

```json
{
  "ok": true,
  "token_results": {
    "<code>": {
      "ok": true,
      "token": "xoxc-...",
      "redir": "https://....slack.com/client/...",
      "auth_redir": "",
      "team": {
        "id": "T12345678",
        "url": "https://....slack.com/",
        "...": "..."
      }
    }
  }
}
```

- `auth_redir` non-empty means "requires an additional browser authentication step" (SSO); mautrix treats that as browser-required.
- Pagination: none.
- Used for: this is the desktop-app "magic login" exchange: the browser opens a magic-login link, Slack hands back per-workspace tokens in one shot (bulk = one code covering several workspaces). This is the closest thing to the official `slack://` hand-off that can be called directly.
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:442-525 (`exchangeMagicLoginCode`: GET `app.slack.com/api/auth.loginMagicBulk?magic_tokens=...&ssb=1`, `token_results`, `auth_redir` handling)

### GET app.slack.com/auth (client credential exchange)

- Transport: `GET https://app.slack.com/auth?app=client&response_type=json&return_to=<uri>&teams=<teamID>` — needs the browser session cookies in the jar; response is JSON.
- Visibility: private.
- Request params:
  - `app` - string - observed `"client"`
  - `response_type` - string - observed `"json"`
  - `return_to` - string - URI path the client wants to land on (e.g. `/client/T...`)
  - `teams` - string - comma-separated team IDs to mint tokens for
- Response:

```json
{ "teams": { "T12345678": { "token": "xoxc-..." } } }
```

- Pagination: none.
- Used for: minting `xoxc` tokens for arbitrary workspaces of an already-authenticated session. If you have a `d` cookie (or any Slack web session), this returns workspace tokens without re-running a login flow — the primary "cookie -> token" recovery path alongside `ssb/redirect`.
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:309-320, 414-440 (`fetchAuthToken`: query `app=client&response_type=json&return_to=...&teams=<id>`, response `teams[id].token`)

### GET <ws>.slack.com/ssb/redirect (d-cookie -> token page)

- Transport: `GET https://<ws>.slack.com/ssb/redirect` with `Cookie: d=<xoxd-value>` and a browser User-Agent. HTML response (not JSON).
- Visibility: private (page, not an API method).
- Request params: none; auth entirely via the `d` cookie.
- Response: an HTML boot page for the signed-in session. The `xoxc` token appears inline as:

```json
"api_token":"xoxc-9-..."
```

(regex `"api_token":"([^"]+)"`). The response also sets fresh session cookies which should be captured together with the token.

- Pagination: none.
- Used for: cookie-only sign-in (slackdump `NewCookieOnlyAuth`): give it the `d` cookie from a browser/desktop import, get back a working `xoxc` + cookies. Equivalent in spirit to the `app.slack.com/auth` exchange but per-workspace and page-scraped.
- OBSERVED: rusq/slackdump/auth/token.go:32-46 (`GET ssb/redirect`, `Cookie: d=` header), :56-89 (`extractToken` regex `"api_token":"([^"]+)"`, capture of response cookies)
- OBSERVED: rusq/slackdump/auth/value.go:70-79 (`NewCookieOnlyAuth` flow)
- Cross-check: mautrix's embedded-JS login auto-clicks the same page's "Use Slack in Browser" link (`.p-ssb_redirect__body` -> `.c-link`) — OBSERVED: mautrix/slack/pkg/connector/login-cookie.go:81-87.

### GET app.slack.com/client (version data for _x_ params)

- Transport: `GET https://app.slack.com/client` with session cookies; HTML response.
- Visibility: private (page).
- Request params: none.
- Response: the client shell page. The `<html>` tag carries the deployment version attributes:

```html
<html
  data-gantry
  data-version-ts="1772314064"
  data-version-hash="912a4723..."
  data-build-version-ts="..."
  data-build-manifest-last-modified="..."
  data-frontend-build-type="current"
  data-app-name-override="..."
></html>
```

- Pagination: none.
- Used for: building the `_x_*` query params every web-client API call carries (see Gotchas). The beeper/slackgo fork fetches this page, parses the attributes, and renders `?_x_id=<hash8>-<epochsec.ms>&_x_version_ts=...&_x_frontend_build_type=...&_x_desktop_ia=4&fp=aa&_x_num_retries=0[&_x_gantry=true][&slack_route=...]` onto every request.
- OBSERVED: beeper/slackgo/versiondata.go:46-77 (`FetchVersionData` GET `app.slack.com/client`, attributes `data-version-ts`:96, `data-app-name-override`:106)
- OBSERVED: beeper/slackgo/versiondata.go:28-45 (`ToQuery`: `_x_id`:31, `_x_version_ts`, `_x_desktop_ia`:35, `fp`, `_x_num_retries`, `_x_gantry`:40, `slack_route`:43)

### api.features

- Transport: `POST https://<ws>.slack.com/api/api.features`, form-encoded, `token` field.
- Visibility: private.
- Request params:
  - `token` - string - required - the xoxc token (this is the point: the request carries it)
- Response: feature-flag map (`{"ok":true,"features":{...}}`, ~60 flags per the local teardown).
- Pagination: none.
- Used for: not for its payload, but as the token carrier: every browser-based login implementation (slackdump/rod, playwright variant, mautrix's JS extractor) watches for the client's first `api.features` request after sign-in and extracts `token` from its query string or multipart body to capture the session.
- OBSERVED: rusq/slackauth/hijacker.go:30-43 (hijack route `*/api/api.features*`), :77-91 (extract `token` form field)
- OBSERVED: rusq/slackdump/auth/browser/client.go (deprecated playwright provider) — `ExpectRequest(uri+"/api/api.features*", ...)` then extract from query/multipart; file rusq/slackdump/auth/browser/client.go, extracted in `auth/browser/extractors.go:20-30` (token regex)
- OBSERVED: .omo/research/findings.md (`api.features` observed during fresh web-client load)
- OBSERVED: mautrix/slack/pkg/connector/login-cookie.go:108-121 (cookie capture regex watches request bodies of `api/(client|experiments|api|users|teams|conversations).*` calls, i.e. the same boot burst)

## Credential acquisition flows (how to obtain the xoxc token + d cookie)

### (a) Embedded-webview sign-in, read localConfig_v2 + d cookie (EZ-Login style)

The web client persists all signed-in teams' tokens in `localStorage["localConfig_v2"]` as `{"teams": {"<teamID>": {"token": "xoxc-...", ...}, ...}}`. Modern Slack does **not** embed the token in served HTML anymore — this localStorage key is the source of truth.

Flow: open `https://app.slack.com/signin` (or `https://<ws>.slack.com/`) in an embedded webview; let the user complete whatever Slack asks (email code, CAPTCHA, SSO, 2FA); after sign-in the client redirects to the workspace client and writes `localConfig_v2`; read it via JS; harvest the `d` cookie from the webview cookie store.

- The `d` cookie is set on domain `.slack.com` for the signed-in account (slackdump fabricates its replacement with `Domain: ".slack.com"`), value `xoxd-...` (mautrix validates the shape `^xoxd-[a-zA-Z0-9/+=]+$`).
- JS cannot read `d` (HttpOnly, per the local teardown) — extraction must go through the cookie store, not `document.cookie`.
- wee-slack's standalone extractor reads `localConfig_v2` from a real browser profile and prints `d=<value>;d-s=<value>` per account — evidence of the exact cookie pair to keep.
- slackauth (slackdump's engine) proves the same flow headlessly: open the workspace sign-in page, let the user log in (manual mode), grab cookies from the browser, hijack the first `api.features` request for the token.
- OBSERVED: gammons/slk/internal/slackdesktop/localstorage.go:17-55 (`localConfig_v2` = `{teams: {id: {token}}}`; "Modern Slack no longer embeds the xoxc token in page HTML")
- OBSERVED: mautrix/slack/pkg/connector/login-cookie.go:60-75 (`ExtractSlackTokenJS` polls `localStorage.localConfig_v2` for `xoxc-`, takes `Object.values(...teams)[0].token`)
- OBSERVED: mautrix/slack/pkg/connector/login-cookie.go:110-137 (captures `token` body field with pattern `^xoxc-.+$` from API request bodies, and cookie `d` on domain `slack.com` with pattern `^xoxd-[a-zA-Z0-9/+=]+$`)
- OBSERVED: wee-slack/wee-slack/extract_token_from_browser.py:307-341 (localConfig_v2 JSON, teams map, skips team IDs starting with `"E"` = Enterprise orgs, prints `d=...;d-s=...`)
- OBSERVED: rusq/slackauth/login_manual.go:29-63 (`Manual`: open sign-in page, wait for hijacked token, `browser.GetCookies()`)
- OBSERVED: rusq/slackauth/slackauth.go:124-131 (pre-sets `OptanonAlertBoxClosed` cookie to skip the cookie-consent banner)

### (b) Official desktop `slack://` / magic-login redirect flow

The official desktop app signs in via magic-login links: Slack emails/pages a `magic-login` URL containing a single-use code; the client exchanges it via `auth.loginMagic` (single) or `GET app.slack.com/api/auth.loginMagicBulk?magic_tokens=...&ssb=1` (bulk, sent with `ssb=1`), receiving per-workspace tokens plus a fresh `d` cookie in the same response cycle. The post-login page also renders a `[data-qa="ssb_redirect_open_in_browser"]` button ("open in browser") which the browser-driven clients auto-click to land in the web client and finish booting.

A GPUI client can: (1) trigger the email-code flow (see signup.* endpoints), (2) exchange the code with `auth.loginMagicBulk`, (3) capture the `d` cookie from the HTTP response jar. Registering an actual `slack://` protocol handler gives parity with "launch in app" buttons but is not required for sign-in.

- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:442-460 (`magic_tokens` + `ssb=1` GET), :473-501 (`token_results`/`auth_redir` semantics)
- OBSERVED: rusq/slackauth/login_auto.go:29 (`[data-qa="ssb_redirect_open_in_browser"]` success-page element), :20 (`[data-qa="sign_in_password_link"]`)
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:197-200 (`magic_login_url`/`magic_login_code`/`sso_required`/`two_factor_required` per workspace from `signin.findWorkspaces`)
- Not observed: the exact `slack://` deep-link grammar the official app registers (see LEADS).

### (c) Import from an installed Slack desktop app (macOS)

The Slack desktop app is Electron/Chromium: per-workspace tokens in a Chromium Local Storage LevelDB, session cookie in a Chromium SQLite cookie DB encrypted with a Keychain key.

- Config dir (macOS): `~/Library/Application Support/Slack`; App Store (sandboxed) build: `~/Library/Containers/com.tinyspeck.slackmacgap/Data/Library/Application Support/Slack`.
- Tokens: LevelDB at `<configdir>/Local Storage/leveldb`, key suffix `localConfig_v2` (Chromium key form `_<origin>\x00<1-byte encoding tag><key>`; value's first byte `0x00` = UTF-16LE, `0x01` = Latin-1), JSON `{teams: {<teamID>: {token}}}`. Copy the DB to temp before opening (Slack holds it locked).
- Workspace list: `<configdir>/storage/root-state.json` with `{"workspaces": {"<id>": {"name","url","domain","id"}}}`.
- Cookie: SQLite `<configdir>/Cookies` (Windows: `Network\Cookies`), row `host_key = '.slack.com' AND name = 'd'`, column `encrypted_value`. Decryption: 3-byte version prefix (`v10`/`v11`), then AES-128-CBC with PBKDF2(key, salt=`"saltysalt"`, iterations=1003 on macOS, 16-byte key), IV = 16 spaces; strip PKCS7; on cookie DB version >= 24 strip a leading 32-byte domain hash from the plaintext. The keychain item is the generic password service **`Slack Safe Storage`** (read via `security find-generic-password -w -s "Slack Safe Storage"`); the App Store build stores its key under account **`Slack App Store Key`**, the standalone build under its own account — enumerate with `security dump-keychain` and try each, because the keys are not interchangeable. Validate the decryption by requiring printable ASCII (wrong key otherwise yields garbage that later surfaces as `invalid_auth`).
- Linux analogue: Keyring/KWallet ("Chromium Safe Storage"), iterations=1, or `peanuts` when no keyring.
- OBSERVED: gammons/slk/internal/slackdesktop/configdir.go:24-28 (macOS paths incl. `com.tinyspeck.slackmacgap` container)
- OBSERVED: gammons/slk/internal/slackdesktop/password_darwin.go:11-27 (service `Slack Safe Storage`, account `Slack App Store Key`, per-build keys), :44-83 (`security dump-keychain` enumeration, `find-generic-password -w`)
- OBSERVED: gammons/slk/internal/slackdesktop/cookie.go:35-63 (version prefix, `v10`/`peanuts` on Linux, keyed always on darwin/windows), :80-95 (`decryptCBC(body, pw, 1003)` on macOS; printable-ASCII validation; DB paths `Cookies` / `Network\Cookies`)
- OBSERVED: gammons/slk/internal/slackdesktop/decrypt.go:30,36 (PBKDF2 salt `saltysalt`)
- OBSERVED: gammons/slk/internal/slackdesktop/localstorage.go:19-55, 96-103 (`localConfig_v2` LevelDB suffix match, encoding tags, temp-copy)
- OBSERVED: gammons/slk/internal/slackdesktop/workspaces.go:36-56 (`storage/root-state.json` `workspaces` map)
- OBSERVED: wee-slack/wee-slack/extract_token_from_browser.py:127-131, 278-296 (Chrome: `Chrome Safe Storage`, PBKDF2 1003 iterations macOS; cookie DB version >= 24 strips 32-byte prefix), :53 (queries cookie names `d` and `ds`)

### (d) Manual paste

User pastes `xoxc-...` + `xoxd-...` copied from browser devtools (or a cURL command). Zero dependencies, works with SSO/enterprise, the universal fallback. mautrix ships exactly this as a login flow (`{"auth_token":"xoxc-...","cookie_token":"xoxd-..."}` or a pasted cURL). Send `Cookie: d=<value>` (URL-escape the value when building the cookie, as slack-go does) and `token=<xoxc>` in the body; validate with `auth.test`.

- OBSERVED: mautrix/slack/pkg/connector/login-cookie.go:38-42, 136-160 (instructions to paste JSON or cURL; `SubmitCookies` builds the client from token+cookie and boots)
- OBSERVED: rusq/slackdump/auth/value.go:33-56 (`NewValueAuth(token, cookie)`: xoxc requires the `d` cookie, adds fabricated `d-s`)
- OBSERVED: wee-slack/wee-slack/slack/slack_workspace.py:274-281 (token type detection: `xoxp-` = oauth, `xoxc-` = "session")

### (e) Email+password endpoints (`auth.signin` chain) — status

The classic chain (`auth.findTeam` -> `auth.findUser` -> `auth.signin` with password, optional 2FA `pin`) exists and is fully implemented in older clients (slack-libpurple, Sup), but no client maintained in 2025-2026 uses it for the web-client flow: mautrix's 2026 email flow is the passwordless code+magic-token chain documented above, and browser-driving tools (slackdump/rod, playwright) log in through the real pages (`sign_in_with_password` page still exists for the browser flow). Treat direct password POSTs as legacy/unmaintained: whether `auth.signin` still works from a plain HTTP client today is unverified here (see LEADS); the safe native flow is the email-code chain, with a webview fallback for CAPTCHA/SSO.

- OBSERVED: dylex/slack-libpurple/slack-auth.c:33-104 (full password chain)
- OBSERVED: arnnis/Sup/src/slices/team-thunks.ts:44-77 (password chain incl. 2FA `pin`)
- OBSERVED: rusq/slackauth/login_auto.go:16-32 (browser sign-in page `sign_in_with_password`; `#email`/`#password` fields; 6-digit `[aria-label="digit N of 6"]` challenge fields under `#enter_code_app_root`; error elements `[data-qa-error="true"]`, `#password_error`)
- OBSERVED: mautrix/slack/pkg/connector/login-email.go:33-56 (2026 flow IDs: email -> captcha -> code -> workspace -> two-factor -> browser; no password step)

### SSO, Enterprise Grid org-level tokens, and multiple workspaces

- Session model: one `d` cookie per Slack **account** on domain `.slack.com`, plus one `xoxc` token **per workspace** (the web client keeps them all in `localConfig_v2.teams`). Tools send only `d` as the cookie and the workspace's `xoxc` as the body token.
- Enterprise Grid: org-level team IDs start with `"E"`; wee-slack treats `id.startswith("E")` as org-level and uses the enterprise id for the edgeapi path; its import script skips E-teams when collecting web-client tokens. emacs-slack keeps both an enterprise token and per-team tokens, using the enterprise token for most endpoints but a hardcoded nine-endpoint list (rtm.connect, conversations.list, files.getUploadURLExternal, files.completeUploadExternal, getPermalink, conversations.members, conversations.open, usergroups.list, conversations.join) with the **team** token. Grid requests add the `slack_route=E<org>:E<org>` query param for routing (observed in the live-client teardown).
- SSO-required workspaces: `signin.findWorkspaces` flags them with `sso_required`; `auth.loginMagicBulk` returns `auth_redir` for them — both mean "finish in a browser/webview", no pure-API shortcut observed.
- Org migration states surface as `org_login_required` / `team_added_to_org` errors (public docs error table).
- OBSERVED: wee-slack/wee-slack/slack/slack_workspace.py:278-281 (`team_is_org_level` = id starts with "E"), :379-385 (`enterprise_id` fallback), slack/slack_api.py:79-80 (edgeapi path uses `enterprise_id or team id`)
- OBSERVED: wee-slack/wee-slack/extract_token_from_browser.py:334-336 (skips `startswith("E")` teams)
- OBSERVED: yuya373/emacs-slack/slack-request.el:211-238 (enterprise token vs team token endpoint list incl. `slack-rtm-connect-url`)
- OBSERVED: .omo/research/gist.md (`slack_route=E03C83N442J:E03C83N442J`, `<workspace>.enterprise.slack.com`, edgeapi `/cache/<EID>/`)
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:199, 489-492 (`sso_required`, `auth_redir` = browser step required)
- OBSERVED: https://docs.slack.dev/reference/methods/auth.test (`enterprise_id` on Grid tokens; `org_login_required`, `team_added_to_org` errors)

### Required headers, cookies, and _x_* params

- `token` field: every workspace API call carries `token=<xoxc>` in the form/JSON body (alternative: `Authorization: Bearer <token>` — wee-slack sends Bearer for session tokens and it works, and the public docs bless the header for public methods).
- `Cookie: d=<xoxd-value>` — the only cookie required for API calls; slackdump/mautrix/slack-go all send exactly this one (fabricating it with `Domain: .slack.com`, `Secure`, long expiry). URL-escape the value (slack-go does `url.QueryEscape`). emacs-slack sends the raw header `Cookie: d=<value>; `.
- `d-s` cookie: session-start timestamp; slackdump fabricates `d-s = now-10` when it only has a token, wee-slack carries the real one from the browser, emacs-slack stores `d`, `d-s` and `lc` cookies for its websocket connection. API calls observed to work without it.
- `User-Agent`: browser-like Chrome UA (`Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36` is slackdump's default). A too-old pinned Chromium gets **rejected on the login page** (slackdump issue #675 workaround: prefer the system browser). wee-slack uses a plain `wee_slack <version>` UA for API calls — UA strictness is a login-page problem, not an API problem.
- `_x_*` query params (web client parity): `_x_id=<hash8>-<epochsec.millis>` (or `noversion-<sec>.<millis>` when no version hash), `_x_version_ts`, `_x_frontend_build_type`, `_x_desktop_ia=4`, `fp`, `_x_num_retries=0`, `_x_gantry=true`, `slack_route=<EID:EID>` (Grid); optional `_x_csid` per-view session id. Form/body fields: `_x_reason` (call-site tag like `webapp_message_send`, `get_started_workspaces`, `client-counts-api/fetchClientCounts`), `_x_mode=online` on writes, `_x_sonic=true` on boot calls. None are strictly required by the API (mautrix sends only `_x_id`+`_x_reason` for login calls), but they are what the server sees from the real client.
- Login-phase calls add `Referer: https://slack.com/signin` and `Accept: application/json`.
- OBSERVED: beeper/slackgo/slack.go:106-116 (`OptionCookie` -> `d` cookie, `url.QueryEscape`, Domain `.slack.com`)
- OBSERVED: mautrix/slack/pkg/connector/client.go:73-80 (`OptionCookie("d", cookieToken)`)
- OBSERVED: rusq/slackdump/auth/value.go:45-57, 100-106 (`makeCookie`: `d` + fabricated `d-s`, 10-year expiry, `.slack.com`)
- OBSERVED: rusq/slackdump/auth/auth.go:144-153 (HTTP client: cookie jar + `User-Agent: slackauth.DefaultUserAgent` + uTLS)
- OBSERVED: rusq/slackauth/useragent.go:10-13, 48 (default UA string format)
- OBSERVED: rusq/slackdump/auth/rod.go:129-142, 211-215 (bundled Chromium ~v128 rejected by Slack login page; use system browser)
- OBSERVED: wee-slack/wee-slack/slack/slack_api.py:66-71 (`useragent: wee_slack <ver>`, `Authorization: Bearer`, cookie header)
- OBSERVED: yuya373/emacs-slack/slack-request.el:46-53 (`xoxc` check; stores `d`, `d-s`, `lc`), :238-242 (`Cookie: d=%s; ` header on API calls)
- OBSERVED: mautrix/slack/pkg/connector/login-email-api.go:583-621 (multipart POST with `_x_reason` field:591, `_x_id` query:601, `Referer` signin:609), :647-652 (`noversion-<sec>.<millis>` id format)
- OBSERVED: beeper/slackgo/versiondata.go:28-45 (`ToQuery` rendering of `_x_*` + `slack_route`)
- OBSERVED: .omo/research/gist.md (`_x_id`, `_x_csid`, `_x_version_ts`, `_x_frontend_build_type`, `_x_desktop_ia`, `_x_gantry`, `fp`, `_x_num_retries` observed live; `_x_reason`/`_x_mode`/`_x_sonic` form fields; `slack_route`)
- OBSERVED: rusq/slackdump/internal/edge/client.go:61, 141 (`webclientReason(...)` = `_x_reason` on private calls)

## Websocket events

### wss://wss-primary.slack.com connect (session-bound realtime)

- Transport: `wss://wss-primary.slack.com/?token=<xoxc>&gateway_server=<teamID>-1&slack_client=desktop&batch_presence_aware=1`, with the `d` cookie attached (clients store `d`, `d-s`, `lc` on `.slack.com` for this connection). wee-slack ditches the public `rtm.connect` URL for session tokens and builds this URL directly; the public method `rtm.connect` (returns the `wss://` url + `self`/`team`) is the cross-area equivalent for bot tokens.
- Events: none in this area were observed; session revocation/expiry is _not_ delivered as a documented event here. Clients detect it via API errors on any call: mautrix maps `invalid_auth`/`not_authed` and `token_revoked`/`token_expired` to its bad-credentials bridge state (`slack-invalid-auth` / `slack-token-revoked`) and tears the session down; the realtime-area doc owns the event vocabulary (`hello`, `goodbye`, ...).
- OBSERVED: wee-slack/wee-slack/slack/slack_workspace.py:388 (session-token websocket URL with `token=`, `gateway_server=<id>-1`, `slack_client=desktop`, `batch_presence_aware=1`)
- OBSERVED: yuya373/emacs-slack/slack-request.el:49-53 (`slack-url-cookie-store`: `d`, `d-s`, `lc` cookies "required cookies for websocket connection")
- OBSERVED: .omo/research/findings.md (`wss://wss-primary.slack.com/...` with `lazy_channels=1`, `no_query_on_subscribe=1` observed live)
- OBSERVED: mautrix/slack/pkg/connector/client.go:230-260 (`handleConnectError` mapping `invalid_auth`, `not_authed`, `token_revoked`, `token_expired`, `account_inactive`, `team_disabled`, `user_removed_from_team`)

## Gotchas

- **Invalidation error codes to handle** (map them to "re-auth" UI): `invalid_auth` (bad/rotated token), `not_authed` (no token), `token_revoked`, `token_expired`, `account_inactive`, `user_removed_from_team`, `team_disabled`, `team_not_found`, `enterprise_is_restricted`, `missing_scope` / `not_allowed_token_type` / `invalid_token_type`, `org_login_required` (enterprise migration), `team_added_to_org` (migration in progress), `two_factor_setup_required`, `ratelimited`. OBSERVED: https://docs.slack.dev/reference/methods/auth.test (error table) and mautrix/slack/pkg/connector/loginerrors.go:104-118 (`invalid_auth`/`not_authed`/`no_permission`, `token_revoked`/`token_expired`, `captcha_required`/`invalid_captcha`/`challenge_failed`, `browser_auth_required`/`sso_required`/`saml_required`).
- **xoxc shape**: `xoxc-<n>-<n>-<n>-<64 hex>` — slackdump validates tokens with `xoxc-[0-9]+-[0-9]+-[0-9]+-[0-9a-z]{64}` and refuses xoxc without a `d` cookie. OBSERVED: rusq/slackdump/auth/auth.go:38, 60-64; mautrix pattern `^xoxc-.+$` OBSERVED: mautrix/slack/pkg/connector/login-cookie.go:117.
- **Cookie value escaping**: the browser stores the `d` value URL-encoded; slack-go escapes it into the cookie, slackdump escapes only if not already URL-safe, emacs-slack sends it raw with a trailing `; `. Double-escaping breaks auth — normalize once at import. OBSERVED: beeper/slackgo/slack.go:112; rusq/slackdump/auth/value.go:96-100; yuya373/emacs-slack/slack-request.el:240.
- **Rate limits**: `auth.test` has special (generous) limits; everything else returns HTTP 429 + `Retry-After` (seconds or HTTP-date) with a JSON body like `{"ok":false,"error":"ratelimited"}`. Email-login steps have their own `code_request_limit_reached` / `ratelimited` codes. slackdump treats missing/invalid `Retry-After` as 5s and caps accepted delays at 1h. OBSERVED: https://docs.slack.dev/reference/methods/auth.test; mautrix/slack/pkg/connector/login-email-api.go:622-640 (429 + body handling); rusq/slackdump/internal/edge/edge.go:263-280 (`defRetryAfter` 5s, `maxRetryAfter` 1h); wee-slack/wee-slack/slack/http.py (429 -> sleep `Retry-After`).
- **Login page is browser-gated**: Slack rejects very old Chromium builds on the sign-in page (slackdump had to switch to the user's system browser); email-code sign-in can demand a CAPTCHA; unknown browsers trigger a 6-digit email challenge code inside the page flow. Any pure-HTTP login path must be prepared to hand off to a webview. OBSERVED: rusq/slackdump/auth/rod.go:129-142, 211-215; rusq/slackauth/login_auto.go:29-32; mautrix/slack/pkg/connector/login-email-api.go:115-126.
- **TLS fingerprint**: slackdump's HTTP client uses uTLS (`chttp.WithUTLS`) for its authenticated requests — suggestive of server-side TLS fingerprinting tolerance being a thing, though no failure mode for plain Rust TLS was observed here. OBSERVED: rusq/slackdump/auth/auth.go:148-152.
- **Tokens are not in HTML**: modern Slack no longer embeds `xoxc` in the boot page; scrapers must use `localConfig_v2` (webview/browser) or the `/ssb/redirect` page's `"api_token"` JSON line. OBSERVED: gammons/slk/internal/slackdesktop/localstorage.go:17; rusq/slackdump/auth/token.go:56-89.
- **Enterprise quirks**: E-prefixed org IDs, `slack_route` routing param, enterprise token vs team token endpoint split, edgeapi path keyed by enterprise id — see the SSO/Grid section above; all cited there.
- **Cookie-consent banner** blocks automated logins: preset `OptanonAlertBoxClosed` (a recent RFC3339 timestamp works). OBSERVED: rusq/slackauth/slackauth.go:124-131; rusq/slackdump/auth/browser/client.go (same trick in the playwright provider).
- **Workspace-existence check**: `HEAD https://<ws>.slack.com/` returns 200/403 for real workspaces and 404 for bogus ones — cheap pre-flight before offering "sign in to <ws>". OBSERVED: rusq/slackauth/slackauth.go:291-302.
- **QR / mobile sign-in** exists as another browser-assisted path: decode the QR URL from the desktop/mobile login screen and open it in a controlled browser, then wait for the token hijack. OBSERVED: rusq/slackauth/mobile_signin.go:15-68 (`QRAuth`, `ErrLinkExpired` on expired link title).
- **Session persistence**: the credential set to persist is exactly `{token, cookie d (, d-s)}` — slackdump serializes a provider to JSON and reloads it; everything else (boot data, prefs) is re-derivable. OBSERVED: rusq/slackdump/auth/auth.go:68-107 (`Load`/`Save`).

## RECOMMENDATION (GPUI app on macOS, embedding WKWebView via `wry`)

Ranking for slackless, best first:

1. **Embedded-webview sign-in (wry WKWebView) at `https://app.slack.com/signin`** — primary path. Renders whatever Slack demands (email code, CAPTCHA, SSO/IdP redirects, 2FA) with zero protocol maintenance, and on success the page itself writes `localConfig_v2.teams[<id>].token` — read it via injected JS and harvest the `d` cookie from the webview cookie store. Tradeoffs: (i) the `d` cookie is HttpOnly, so the cookie must come from `WKWebsiteDataStore`/wry's cookie API, not `document.cookie` — verify wry's cookie-read support on the pinned version (LEADS); (ii) first integration cost is gluing wry into a gpui window and intercepting navigation; (iii) users see a browser-style login, which is exactly what the official app does anyway. This is the only path that keeps working when Slack ships a new CAPTCHA or SSO flow.
2. **Manual paste of `xoxc` + `d`** — must-have fallback (power users, headless setups, CI). Trivial: two text fields, normalize cookie escaping, validate with `auth.test`. Tradeoff: poor discoverability (requires devtools literacy), no SSO-handholding.
3. **Import from installed Slack desktop app** — best "aha" UX for Slack users (zero login), fully offline-capable, multi-workspace at once via `localConfig_v2` + `root-state.json`. Tradeoffs: Keychain prompt (`Slack Safe Storage`, possibly several candidate keys for App Store vs standalone builds), LevelDB/SQLite format churn when Slack updates its app, sandbox-container path differences, and a scarier security narrative ("slackless read my Slack keychain item"). Ship it as convenience, not the default.
4. **Native email-code chain** (`signup.checkEmail` -> `auth.captcha` -> `signup.confirmEmail` -> `signin.confirmCode` -> `signin.findWorkspaces` -> `auth.loginMagic`/`auth.loginMagicBulk`) — the only fully-native (no webview) login; good second-stage polish for keyboard-first UX. Tradeoffs: CAPTCHA sitekey must be rendered somewhere (embed a tiny webview just for the challenge — at which point path 1 exists), SSO workspaces still fall back to a browser step (`sso_required`, `auth_redir`), and it is the most churn-prone private surface (five endpoints instead of none).
5. **Password `auth.signin` chain** — do not build. Legacy, no maintained client uses it, captcha/challenge risk, and password handling raises the security bar for no gain.
6. **`slack://` handler registration** — do it late, for parity with "open in desktop app" buttons (receive magic-login links, route into path 4's `auth.loginMagicBulk`); never as a sign-in source.

Supporting advice: persist exactly `{token, d}` (+`d-s` if available); re-derive a lost token from a live `d` cookie via `app.slack.com/auth` (JSON, clean) or `ssb/redirect` (HTML scrape, fallback); send `Cookie: d=...` + body `token=` + a browser UA + the `_x_id`/`_x_version_ts` set from `app.slack.com/client`; treat `invalid_auth`/`token_revoked` as "start path 1" everywhere.

## LEADS

(unverified beliefs and open questions — verify before relying)

- Whether `auth.signin` (password POST) still succeeds from a plain HTTP client today; the sign-in _page_ still links a password form (`[data-qa="sign_in_password_link"]`, observed in rusq/slackauth/login_auto.go:20), but no maintained client exercises the API directly.
- wry crate: does it expose cookies (`cookies_for_url` / page->`WKWebsiteDataStore`) for HttpOnly cookies, and on which version? Decides how path 1 extracts `d`. (gpui-component's webview wrapper may add this too.)
- The exact `slack://` deep-link grammar (`slack://<team>/generic/...`, magic-login payload shape) the official desktop app registers; unobserved in any mined source.
- `_x_app_name`: the task brief mentions it; beeper/slackgo parses `data-app-name-override` (versiondata.go:106) but never emits an `_x_app_name` param — real web client may send it; capture from a live session to confirm.
- `auth.emailToken`, `auth.magicLogins.create`, `auth.enterpriseSignout`, `auth.removePendingTeam`, `signup.checkSignupDomains`, `signup.confirmEmail` variants — method names confirmed only via 3kh0/slack-datamine/build/api-methods.txt; parameters/responses unknown.
- Real lifetimes: how long `d` and `xoxc` actually live (days? months?), whether Slack rotates `xoxc` while `d` stays valid, and whether the short-lived `x` cookie (15 min, per .omo/research/gist.md) ever matters for API calls. Fabrications observed (10-year cookie expiry) are client conveniences, not server facts.
- Magic-login code TTL and single-use semantics (`expired_code`, `code_already_used` observed as error names only).
- Whether plain (non-uTLS) Rust TLS gets blocked on login endpoints under load; slackdump's uTLS choice is circumstantial.
- `lc` cookie semantics (emacs-slack stores it for websocket; nobody documents it).
- Websocket-level session teardown: whether revocation pushes a specific frame (`goodbye`?) or just drops the connection — realtime-area doc should confirm.
- wee-slack queries a cookie literally named `ds` (extract_token_from_browser.py:53) while every other source says `d-s` — likely a wee-slack bug; double-check the actual Chromium cookie name on a real profile.
- Sup's `API_URL` env value (its `auth.findTeam`/`auth.signin` base host) — file src/utils/http/index.ts:2 imports it from `../../env`; value not inspected.
- `auth.test` for `xoxc` may return extra web-client fields beyond the public docs (guest channels etc.) — capture a live response to enrich the excerpt.
- `_x_id` query param on `auth.findTeam`: previously claimed "added by web clients". unverified: no cited source shows it — Sup and slack-libpurple send no `_x_id` on this call, and mautrix never calls `auth.findTeam`.
- `auth.loginMagic` response token prefix `xoxs-`: unverified: mautrix `nativeMagicLogin` decodes the token without checking its prefix (mautrix/slack/pkg/connector/login-email-api.go:387-392); tokens mautrix harvests from browser-boot traffic are `xoxc-` (`slackAuthTokenPattern`, login-email-api.go:45).
