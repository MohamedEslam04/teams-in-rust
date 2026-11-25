# Spike: personal-account (Teams free) sign-in

Gate 0 of the personal-account work (`concepts_brief.md`, "Auth feasibility (the gate)" and
"Author decisions", decision 2). The spike answers one question: can IronTeams get from a personal
Microsoft account (MSA) to a Teams free chat list with a desktop-friendly sign-in? It is a CLI
subcommand only. It does not change the desktop app or any work-account code path, and it stores
no token.

Code: `crates/ironteams-cli/src/spike.rs`.

## How to run it

```text
ironteams-cli spike personal-login [--client office|azcli] [--out FILE]
```

1. Run `ironteams-cli spike personal-login --out personal-auth.json`.
2. The command prints a verification URL and a short code. Open microsoft.com/devicelogin
   yourself, enter the code and sign in with your own personal account. Sign in once.
3. The command polls the token endpoint until you finish, decline, or the code expires
   (about 15 minutes). It then runs stages 2 and 3 and prints one GO/NO-GO line per stage.
4. Read the transcript before you share it or copy facts from it into a doc or fixture.

`--client office` (the default) uses the Microsoft Office public client
`d3590ed6-52b3-4102-aeff-aad2292ab01c`. Use `--client azcli` (Azure CLI,
`04b07795-8ddb-461a-bbee-02f9e1bf7b46`) only if Office is refused, and only after a pause.
Never run it in a loop.

The binary: `ironteams-cli` is not in the release artifacts. Build it with
`cargo build -p ironteams-cli` on a machine that has the memory for it. Do not build it on the
development box, which freezes under cargo.

## What each stage proves

| Stage | Request | GO means | What it prints |
|---|---|---|---|
| 1. token | `POST https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode` with scope `https://auth.fl.teams.microsoft.com/teams.auth.readwrite offline_access`, then device-code polls on `…/consumers/oauth2/v2.0/token` | A first-party public client can get a Teams-audience token for a personal account through device code (flow rung D in the brief). | Token answer field names, granted scopes, `expires_in`, whether a refresh token came back and its `refresh_token_expires_in`. When the access token is a JWT: `tid` (marked if it is the personal-account tenant `9188040d-…`), `aud`, and the claim names (not their values). |
| 2. skypetoken | `POST https://teams.live.com/api/auth/v2.0/authz/consumer` with the work `authz` headers (`User-Agent`, `Authorization: Bearer`, `Accept: application/json; ver=1.0`, `Content-Length: 0`) | The consumer authz service accepts this client's token and issues a skypetoken. | HTTP status, the answer's key names, whether a skypetoken is present and at which path, `expiresIn`, the `regionGtms` key names with the JSON type of each value (string or `{endpointUrl}` object), and the chat-service URLs (hosts, not secrets). On failure, the error body cut to 300 characters with tokens, emails and ids redacted. |
| 3. chat list | `GET {base}/v1/users/ME/conversations?view=msnp24Equivalent&pageSize=5`, first on `https://msgapi.teams.live.com`, then on the chat hosts from `regionGtms` | The skypetoken reads the consumer chat service, so the chat stack can be reused with consumer hosts. | Per host: HTTP status. Per conversation (5 at most): the id shape (`19:…@thread.v2`, `48:notes`), the thread type and the member count. Never names, topics or message text. |

The skypetoken is only sent to `https://msgapi.teams.live.com` or to
`https://teams.live.com/api/chatsvc/consumer`, matched exactly (no suffix match, https, default
port). A different host named in `regionGtms` is listed as skipped and gets nothing. The spike's
HTTP client follows no redirects, so a token cannot follow one to another host.

All three GO means flow D works up to a chat list. It does not settle the session length: the
brief's refresh questions (+1 h, +25 h, rotation) and the per-audience scopes are the next live
checks.

## The transcript (`--out`)

A JSON file for this doc and for the P2 fixtures. It holds status codes, request and response
header names, key names, field presence, scopes, lifetimes, `regionGtms` key names, value shapes
and chat-service URLs, and the shape-only conversation rows. It never holds a token, the device
or user code, a tenant id (only `tid_is_consumer_tenant`), names or message text. The file is
also passed through the same token scrubber as the terminal output.

## Where the tokens go

Nowhere. The access token, the refresh token and the skypetoken stay in memory for the length of
the run. Nothing is written to the keyring or the config, and the existing work-account session is
not read or touched. The refresh token is not kept at all; the spike only records whether one came
back.

## Risk notes

- **Unofficial use of a first-party client id.** Office and Azure CLI are Microsoft's own public
  clients. Using them to reach Teams free is not a documented or supported integration. Microsoft
  can change which scopes they may request, or block them, without notice. A GO from the spike is
  evidence for a flow, not a promise that it will keep working.
- **One sign-in.** The author signs in once, with their own personal account, typing the code on
  microsoft.com/devicelogin themselves (author decision 2). The password is never scripted. Stop at
  the first "unusual sign-in" email. A flagged personal account loses mail, OneDrive and Xbox, not
  just Teams.
- **Never automate retries.** Each request is sent once. The device-code poll is the protocol's own
  wait (it honours `interval` and `slow_down`), not a retry. Every failed token request is a
  login-abuse signal, so do not rerun the command in a loop, script it, or switch clients straight
  after a refusal. If a stage fails, record the result here and decide on the next step by hand.

## Results

Not run yet. Record the date, the client used, the three GO/NO-GO lines and a link to the scrubbed
transcript here.
