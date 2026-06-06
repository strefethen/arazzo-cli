# OAuth Loopback Support Plan (Implementation-Ready)

## Level Of Effort Assessment

- MVP (authorization code + PKCE login, client credentials grant, API key passthrough, callback server, token exchange, run-time bearer injection): **6-8 engineering days**
- Strong v1 (secure storage, refresh-on-expiry, OIDC discovery, credential backends, project-level config, robust tests/docs): **12-15 engineering days**
- Extended scope (device-code fallback, multi-provider UX polish): **2-3 weeks**

Assumptions:
- One engineer.
- No UI beyond browser open / URL print.
- Existing runtime/CLI architecture remains intact (feature lands mostly in `arazzo-cli` crate with a thin runtime integration seam).

## 1. Scope Lock (v1)

### In Scope
- OAuth 2.0 Authorization Code flow with PKCE (public client model).
- OAuth 2.0 Client Credentials Grant (machine-to-machine, service accounts).
- API key authentication (static header or query parameter injection).
- Local loopback callback server for CLI login (`127.0.0.1`, ephemeral port).
- Token exchange and secure token storage.
- Access token refresh when expired (with configurable buffer).
- OIDC Discovery (`.well-known/openid-configuration` auto-resolution).
- Client secret support (for confidential clients and client credentials grant).
- Credential backend abstraction (keychain, encrypted file, env vars).
- Project-level `.arazzo-auth.yaml` config file.
- `run --auth-profile <name>` header injection.
- `run --api-key-header <name>=<value>` header injection.
- `--json` output for all auth commands.

### Out Of Scope (v1)
- Device code grant.
- Team/shared credential stores.

## 2. User-Facing CLI Contract

### New Command Group
```bash
arazzo-cli auth login <profile> --client-id <id> --auth-url <url> --token-url <url> --scope <scope>...
arazzo-cli auth login <profile> --client-id <id> --client-secret <secret> --token-url <url> --grant client_credentials --scope <scope>...
arazzo-cli auth login <profile> --issuer <url> --client-id <id>   # OIDC discovery mode
arazzo-cli auth status <profile>
arazzo-cli auth logout <profile>
```

### Run Integration
```bash
arazzo-cli run <spec> <workflow-id> --auth-profile <profile>
arazzo-cli run <spec> <workflow-id> --api-key-header X-Api-Key=<value>
arazzo-cli run <spec> <workflow-id> --api-key-query api_key=<value>
```

### Optional Flags
- `auth login`
  - `--no-open` (print authorization URL instead of launching browser)
  - `--callback-timeout <duration>` (default `120s`)
  - `--audience <value>` (provider-specific, optional)
  - `--issuer <url>` (OIDC discovery base URL)
  - `--grant <type>` (`authorization_code` default, or `client_credentials`)
  - `--client-secret <value>` (for confidential clients; prefer `ARAZZO_CLIENT_SECRET` env var)
  - `--credential-backend <backend>` (`keychain`, `encrypted-file`, `env`; default `keychain`)
- `run`
  - `--auth-profile <profile>`
  - `--api-key-header <name>=<value>` (inject static API key as request header)
  - `--api-key-query <name>=<value>` (inject static API key as query parameter)

### OIDC Discovery

When `--issuer <url>` is provided, the CLI fetches `<url>/.well-known/openid-configuration` and extracts `authorization_endpoint`, `token_endpoint`, and optionally `revocation_endpoint`. This removes the need to supply `--auth-url` and `--token-url` manually.

Discovery failures produce the `AUTH_DISCOVERY_FAILED` error code. The fetched metadata is cached in the profile so subsequent operations do not re-fetch.

## 3. Security Requirements

- Bind callback server only to `127.0.0.1`.
- Generate cryptographically strong `state`.
- Use PKCE verifier/challenge with `S256`.
- Validate exact `state` match on callback.
- One callback request max; then shut down server (see Callback Server Edge Cases below).
- Never print, trace, or serialize raw tokens.
- Keep tokens only in the configured credential backend (keychain by default; never plaintext config).
- Client secrets: accept via `--client-secret` flag or `ARAZZO_CLIENT_SECRET` env var. Never persist the secret to disk in plaintext — store in the same credential backend as tokens. Warn on stderr if `--client-secret` is passed as a CLI arg (prefer env var to avoid shell history leakage).
- Reuse existing trace redaction safeguards for any auth-related request fields (see Redaction Integration below).

### Callback Server Edge Cases

Per RFC 8252 Section 7.3 and real-world portability:

- **Port retry on bind failure**: If the initial ephemeral port bind fails (e.g., lingering TIME_WAIT from a previous run), retry up to 3 times on different ephemeral ports before emitting `AUTH_CALLBACK_BIND_FAILED`.
- **WSL2 support**: Detect WSL2 via `/proc/version` containing `microsoft`. Under WSL2, use `wslview` or `xdg-open` for browser launch instead of `webbrowser` crate defaults.
- **Favicon handling**: The callback server must respond to `GET /favicon.ico` with `204 No Content` to prevent browsers from sending a second request that could interfere with the one-callback-max logic.
- **Success HTML page**: On a valid callback, respond with a minimal HTML page (`200 OK`) containing a "You may close this tab" message, rather than a raw text body.
- **CSRF via open redirect**: Validate that the `redirect_uri` in the callback exactly matches the one sent in the authorization request.

## 4. Persistence Model

### Profile Metadata (non-secret)
- Stored in user config directory:
  - `~/.config/arazzo/auth/profiles.json` (platform-equivalent via `dirs` crate)
- Fields:
  - `profile`
  - `clientId`
  - `authUrl`
  - `tokenUrl`
  - `scopes[]`
  - `grantType` (`authorization_code` | `client_credentials`)
  - `issuer` (optional, OIDC discovery origin)
  - `credentialBackend` (`keychain` | `encrypted-file` | `env`)
  - `createdAt`, `updatedAt`

### Token Material (secret) — 3-Tier Credential Backend

The credential backend is selectable per profile via `--credential-backend`:

1. **`keychain`** (default) — OS keychain via `keyring` crate.
   - service: `arazzo-cli`
   - account: `<profile>`
   - Preferred for interactive developer machines.

2. **`encrypted-file`** — AES-256-GCM encrypted JSON file at `~/.config/arazzo/auth/<profile>.enc`.
   - Encryption key derived from a passphrase via Argon2id (prompted once, cached in memory for the session).
   - Suitable for environments without a keychain daemon (headless Linux, containers).

3. **`env`** — Read token directly from environment variable `ARAZZO_TOKEN_<PROFILE>` (uppercased profile name).
   - Write operations are no-ops (token is externally managed).
   - Ideal for CI/CD pipelines where tokens are injected via secrets.

All backends implement a common `CredentialStore` trait:
```rust
trait CredentialStore {
    fn load(&self, profile: &str) -> Result<TokenData>;
    fn save(&self, profile: &str, token: &TokenData) -> Result<()>;
    fn delete(&self, profile: &str) -> Result<()>;
}
```

Secret payload (JSON shape across all backends):
- `accessToken`
- `refreshToken` (optional)
- `expiresAt` (epoch ms or RFC3339)
- `tokenType` (expect `Bearer`)
- `clientSecret` (optional, stored alongside token for confidential clients)

### Project-Level `.arazzo-auth.yaml`

A project can include a `.arazzo-auth.yaml` file (next to the Arazzo spec or at repo root) to declare auth profiles declaratively:

```yaml
profiles:
  staging:
    grant: authorization_code
    client_id: my-client-id
    issuer: https://auth.example.com
    scopes: [openid, profile]
    credential_backend: keychain

  ci-service:
    grant: client_credentials
    client_id: svc-client
    token_url: https://auth.example.com/oauth/token
    scopes: [api:read]
    credential_backend: env
```

Resolution order: CLI flags > `.arazzo-auth.yaml` > `~/.config/arazzo/auth/profiles.json`.

The file never contains secrets. Secrets come from the credential backend or env vars.

## 5. Architecture Changes (File-by-File)

1. `crates/arazzo-cli/src/cli.rs`
- Add `Auth` command group and subcommands (`Login`, `Status`, `Logout`).
- Add `--auth-profile`, `--api-key-header`, `--api-key-query` to `run`.

2. `crates/arazzo-cli/src/main.rs`
- Route new auth commands to handlers.
- Pass `auth_profile` / API key options into run context/options.

3. `crates/arazzo-cli/src/run_context.rs`
- Extend `RunOptions` with `auth_profile: Option<String>`.
- Extend `RunOptions` with `api_key_headers: Vec<(String, String)>` and `api_key_query: Vec<(String, String)>`.

4. `crates/arazzo-cli/src/handlers.rs`
- Add:
  - `auth_login(...)`
  - `auth_status(...)`
  - `auth_logout(...)`
- Integrate auth profile resolution into `run_workflow(...)`:
  - load token
  - refresh if needed (with buffer)
  - inject `Authorization: Bearer ...` into default headers before `EngineBuilder`.
- Integrate API key injection:
  - inject `--api-key-header` values into default headers.
  - inject `--api-key-query` values into query parameters for every request.

5. `crates/arazzo-cli/src/output.rs`
- Add auth JSON envelopes and human output emitters:
  - `AuthLoginOutput`
  - `AuthStatusOutput`
  - `AuthLogoutOutput`
  - standard error envelope parity with existing commands.

6. `crates/arazzo-cli/src/auth/` (new module directory)

Split into focused sub-modules instead of a single `auth.rs`:

```text
crates/arazzo-cli/src/auth/
  mod.rs          Re-exports, CredentialStore trait definition
  pkce.rs         PKCE verifier/challenge generation, state nonce
  callback.rs     Loopback HTTP callback server (tiny_http)
  exchange.rs     Token exchange, refresh, client-credentials grant
  discovery.rs    OIDC .well-known fetch + cache
  profile.rs      Profile CRUD (profiles.json read/write)
  keychain.rs     CredentialStore impl: OS keychain via keyring
  encrypted.rs    CredentialStore impl: AES-256-GCM encrypted file
  env_store.rs    CredentialStore impl: env-var passthrough
  api_key.rs      API key header/query injection helpers
```

Each sub-module is independently testable. `mod.rs` exposes the public API consumed by `handlers.rs`.

7. `crates/arazzo-cli/src/main.rs` + schema plumbing
- Extend `schema` command support for auth outputs.
- Add generated schema docs under `docs/schemas/`.

8. `README.md`
- Add auth setup and run examples.
- Add security notes.

## 6. Runtime Command Flow

### `auth login` (Authorization Code + PKCE)
1. Validate CLI args. If `--issuer` is provided, run OIDC discovery to resolve `auth_url` and `token_url`.
2. Create PKCE verifier/challenge + `state`.
3. Start callback server at `http://127.0.0.1:<port>/callback` (with port retry, favicon handling, success HTML page).
4. Build authorization URL with:
   - `response_type=code`
   - `client_id`
   - `redirect_uri`
   - `scope`
   - `state`
   - `code_challenge`
   - `code_challenge_method=S256`
5. Open browser unless `--no-open` (WSL2-aware).
6. Wait for callback (up to `--callback-timeout`).
7. Validate callback `state`.
8. Exchange `code` at token endpoint (include `client_secret` if confidential client).
9. Persist profile metadata + token secret via configured credential backend.
10. Emit success JSON/text.

### `auth login` (Client Credentials Grant)
1. Validate CLI args (`--grant client_credentials` requires `--client-id`, `--token-url` or `--issuer`, and `--client-secret` or `ARAZZO_CLIENT_SECRET`).
2. POST to token endpoint with `grant_type=client_credentials`, `client_id`, `client_secret`, `scope`.
3. Persist profile metadata + token secret.
4. Emit success JSON/text.

### `run --auth-profile`
1. Load profile metadata (CLI flags > `.arazzo-auth.yaml` > user config).
2. Load token from credential backend.
3. If expired (or within refresh buffer) and refresh token exists, refresh token (see Token Refresh Flow below).
4. Persist refreshed token.
5. Inject `Authorization` header for all requests in this run.
6. Continue existing run flow unchanged.

### `run --api-key-header` / `--api-key-query`
1. Parse key=value pairs from CLI flags.
2. For `--api-key-header`: inject as default headers into `ClientConfig`.
3. For `--api-key-query`: append as query parameters to every outgoing request URL.
4. Continue existing run flow unchanged.

### Token Refresh Flow (Detailed)

```
  ┌──────────────────────────────────────────────────┐
  │ Load token from credential backend               │
  └──────────────┬───────────────────────────────────┘
                 │
       ┌─────────▼──────────┐
       │ expires_at - now()  │
       │ > refresh_buffer?   │
       └────┬──────────┬─────┘
          yes          no
            │           │
            ▼           ▼
       Use as-is   ┌─────────────────────┐
                   │ refresh_token exists?│
                   └────┬──────────┬─────┘
                      yes          no
                        │           │
                        ▼           ▼
                   POST /token   Return AUTH_TOKEN_EXPIRED
                   grant_type=   (user must re-login)
                   refresh_token
                        │
               ┌────────▼────────┐
               │ Refresh success?│
               └───┬────────┬────┘
                 yes         no
                   │          │
                   ▼          ▼
              Save new    Return AUTH_TOKEN_REFRESH_FAILED
              token       (user must re-login)
```

The refresh buffer defaults to 30 seconds. This means a token is refreshed when `expires_at - now() < 30s`, avoiding race conditions where a token expires mid-request.

### `auth status`
- Show profile exists, token presence, expiry freshness (without showing token).
- Show credential backend in use and grant type.

### `auth logout`
- Remove token from credential backend.
- Optionally keep metadata unless `--delete-profile` (optional follow-up flag).

## 7. Error Model (Stable Codes)

Recommended command-level error codes:
- `AUTH_PROFILE_NOT_FOUND`
- `AUTH_PROFILE_INVALID`
- `AUTH_CALLBACK_BIND_FAILED`
- `AUTH_CALLBACK_TIMEOUT`
- `AUTH_CALLBACK_STATE_MISMATCH`
- `AUTH_TOKEN_EXCHANGE_FAILED`
- `AUTH_TOKEN_REFRESH_FAILED`
- `AUTH_TOKEN_EXPIRED` — refresh token absent or revoked; user must re-login
- `AUTH_DISCOVERY_FAILED` — OIDC `.well-known` fetch or parse failed
- `AUTH_CREDENTIAL_BACKEND_UNAVAILABLE` — selected backend (keychain daemon, encrypted file key) is not accessible
- `AUTH_KEYCHAIN_READ_FAILED`
- `AUTH_KEYCHAIN_WRITE_FAILED`
- `AUTH_KEYCHAIN_DELETE_FAILED`
- `RUN_AUTH_PROFILE_INVALID`

## 8. Redaction Integration

Auth-related values must be redacted from all trace, debug, and error output. Integration with the existing redaction system:

- **Authorization header**: The existing `redact_headers()` function in `crates/arazzo-runtime/src/runtime_core/helpers.rs` already redacts `Authorization`. Verify coverage for all auth injection paths.
- **Token values**: Any `accessToken`, `refreshToken`, or `clientSecret` value must be redacted if it appears in structured trace output. Add these field names to the runtime's `SENSITIVE_FIELDS` list.
- **CLI output**: `auth status` must never print raw token values. Display only `Bearer ***` or similar masked representation.
- **Error messages**: Token exchange and refresh error responses from the IdP may include token fragments in error descriptions. Sanitize IdP error bodies before including them in CLI error output.
- **`--dry-run` mode**: When `--auth-profile` is combined with `--dry-run`, the planned request output must show `Authorization: [REDACTED]` rather than the actual bearer token.

## 9. Runtime Integration Seam

For v1, auth lives entirely in the CLI crate. However, the design should leave a clean seam for future runtime-level auth if needed (e.g., per-step auth profiles, spec-declared security schemes).

### Current Integration Point (v1)

Auth resolves to HTTP headers before the engine starts. The injection point is in `handlers.rs`, where the resolved `Authorization` header (or API key header) is added to `ClientConfig.default_headers` before constructing the `EngineBuilder`:

```rust
// In handlers.rs run_workflow():
let mut default_headers = cli_headers.clone();
if let Some(profile) = &opts.auth_profile {
    let token = auth::resolve_and_refresh(profile)?;
    default_headers.push(("Authorization".into(), format!("Bearer {}", token.access_token)));
}
for (k, v) in &opts.api_key_headers {
    default_headers.push((k.clone(), v.clone()));
}
let config = ClientConfig { default_headers, ..Default::default() };
```

### Future Runtime Seam (v2+)

If per-step or per-source-description auth is needed later, the runtime would accept an `AuthResolver` trait:

```rust
trait AuthResolver: Send + Sync {
    fn resolve(&self, profile: &str) -> Result<Vec<(String, String)>>;
}
```

This trait would be passed into `EngineBuilder` and called per-request. The CLI would provide an implementation backed by the same credential backend infrastructure. No runtime changes are needed for v1.

## 10. Dependencies

Precise crate additions for `crates/arazzo-cli/Cargo.toml`:

| Crate | Version | Purpose | Feature flags |
|-------|---------|---------|---------------|
| `keyring` | `3` | OS keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service) | — |
| `dirs` | `5` | XDG / platform config directory resolution | — |
| `sha2` | `0.10` | PKCE S256 challenge hash | — |
| `base64` | `0.22` | PKCE challenge + token payload encoding | — |
| `rand` | `0.8` | Cryptographic state + verifier generation | — |
| `webbrowser` | `1` | Open authorization URL in default browser | — |
| `tiny_http` | `0.12` | Loopback callback server (already a dev-dependency in workspace) | — |
| `aes-gcm` | `0.10` | Encrypted-file credential backend (AES-256-GCM) | `aead` |
| `argon2` | `0.5` | Key derivation for encrypted-file backend | — |

No changes required in `arazzo-runtime` for v1 — auth is fully resolved in the CLI layer before engine construction.

## 11. Testing Plan

### Unit Tests (`auth/` sub-modules)
- `pkce.rs`: Verifier/challenge generation format and deterministic length. State generation uniqueness and entropy properties.
- `exchange.rs`: Authorization URL contains required parameters. Expiry/refresh decision logic with buffer.
- `profile.rs`: Profile serialization/deserialization. `.arazzo-auth.yaml` parsing and resolution order.
- `discovery.rs`: OIDC metadata parsing, missing field handling, cache behavior.
- `callback.rs`: Favicon 204 response. Success HTML page content. State validation.

### Integration Tests (`crates/arazzo-cli/tests/cli_integration.rs`)
- `auth login --no-open` flow using mock auth/token server and loopback callback simulation.
- `auth login --grant client_credentials` flow using mock token server (no browser, no callback).
- `run --auth-profile` injects bearer header (assert via local HTTP test server).
- `run --api-key-header` injects custom header (assert via local HTTP test server).
- Expired token triggers refresh request and continues run.
- Missing profile returns structured error code.
- `auth logout` removes token and status reflects signed-out state.
- Credential backend selection (`--credential-backend env`) reads from env var.

### Headless CI Considerations

Keychain-based tests require a running keychain daemon, which is unavailable in most CI environments. Strategy:

- **CI default**: Tests that touch the keychain are gated behind `#[cfg(feature = "keychain-tests")]` or `#[ignore]` with a `ARAZZO_TEST_KEYCHAIN=1` env var opt-in.
- **CI matrix**: macOS runners enable keychain tests (Keychain Access is available). Linux runners use the `env` backend for integration tests.
- **Mock backend**: A `MockCredentialStore` implementing `CredentialStore` is used for all unit tests that exercise token load/save/delete logic without touching the real keychain.
- **Encrypted-file backend**: Tested in all CI environments since it has no platform dependencies beyond the filesystem.

### Schema Drift
- Extend `schema_drift.rs` for auth schema outputs.

### Snapshot Tests
- Add auth JSON output snapshots to contract suite.

## 12. Delivery Plan (Day-by-Day)

### Day 1
- CLI surfaces (`auth` commands, `run --auth-profile`, `--api-key-header`, `--api-key-query`).
- Output type scaffolding and schema placeholders.
- `CredentialStore` trait and `env` backend (simplest first).

### Day 2
- Implement PKCE/state helpers (`pkce.rs`) and loopback callback server (`callback.rs`).
- Implement authorization URL generation.
- Favicon handling, success HTML page, port retry logic.

### Day 3
- Implement token exchange + client credentials grant (`exchange.rs`).
- Implement profile persistence (`profile.rs`) + keychain backend (`keychain.rs`).
- Wire `auth login/status/logout` for authorization code flow.

### Day 4
- Wire `auth login --grant client_credentials`.
- Implement OIDC discovery (`discovery.rs`).
- Implement `.arazzo-auth.yaml` config file loading and resolution order.

### Day 5
- Integrate auth profile into `run` flow.
- Implement refresh-on-expiry logic with buffer.
- Implement API key injection (header and query).
- Redaction integration and verification.

### Day 6
- Encrypted-file credential backend (`encrypted.rs`).
- Integration tests for login callback + run header injection.
- JSON schema generation and drift tests.

### Day 7
- Headless CI test infrastructure (`MockCredentialStore`, feature-gated keychain tests).
- Integration tests for client credentials, OIDC discovery, API key flows.
- Edge-case hardening (timeouts, malformed callbacks, WSL2 detection, credential backend errors).

### Day 8 (buffer)
- README/docs updates.
- UX polish, error message clarity, minor refactors.

## 13. Risks and Mitigations

- Risk: callback never arrives.
  - Mitigation: explicit timeout, clear remediation message, print URL if browser launch fails. Retry port bind up to 3 times.
- Risk: provider-specific quirks.
  - Mitigation: keep first cut standards-based; add provider flags incrementally. OIDC discovery reduces manual configuration.
- Risk: token leakage in logs/traces.
  - Mitigation: no token logging, redact auth headers, test for leakage. Integrate with existing `redact_headers()` and `SENSITIVE_FIELDS`.
- Risk: platform keychain differences.
  - Mitigation: `CredentialStore` trait abstraction with three backends. CI uses `env` backend to avoid keychain dependency. Keychain tests opt-in per platform.
- Risk: client secret exposure in shell history.
  - Mitigation: prefer `ARAZZO_CLIENT_SECRET` env var over `--client-secret` flag. Warn on stderr when flag is used.
- Risk: WSL2 browser launch failure.
  - Mitigation: detect WSL2 and use `wslview`; fall back to `--no-open` with printed URL.

## 14. Definition Of Done

- `arazzo-cli auth login/status/logout` implemented and documented (authorization code + client credentials).
- `arazzo-cli run --auth-profile` works with automatic refresh (with buffer).
- `arazzo-cli run --api-key-header` / `--api-key-query` works for static API key injection.
- OIDC discovery via `--issuer` resolves endpoints automatically.
- Tokens stored securely via configurable credential backend (keychain default).
- `.arazzo-auth.yaml` project-level config supported.
- All auth commands support `--json`.
- Stable error codes shipped and tested (including `AUTH_TOKEN_EXPIRED`, `AUTH_DISCOVERY_FAILED`, `AUTH_CREDENTIAL_BACKEND_UNAVAILABLE`).
- Redaction verified for all auth-related fields in trace/debug/error output.
- Auth sub-modules independently testable; headless CI strategy in place.
- Docs updated with secure usage guidance.
- Verification gates pass:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test --workspace`
