# Antigravity: what Fusebox still lacks (research, 21 November 2026)

Read-only research. Nothing in the Fusebox repo or on the production server was changed.
Fusebox checkout: `main` at 7b65627 (deployed to production 2026-11-21 10:26 CET).

Sources compared:

- **CLIProxyAPI (CPA)** at v8.0.80 (2026-11-20), plus three later commits (HEAD f5c5681a).
  - Cloned to `/scratch/tmp/john/agr-research-20261121/cpa`.
  - Read: the Antigravity executor (`internal/runtime/executor/antigravity_executor*.go`), model probes (`sdk/cliproxy/antigravity_models.go`, `sdk/auth/antigravity_models.go`), version/UA (`internal/misc/antigravity_version.go`), the model registry (`internal/registry/models/models.json`), and the v8.0.73 to v8.0.80 release notes.
- **T3 Code**: the installed binary `~/.t3/runtime/versions/0.1.12-nightly.20261121.5038/t3` (bundled JS, read as data).
  - There is no T3 source checkout. Everything under `/data/t3-worktrees/dev/*` is the Tiaris repo.
- **Gemini CLI**, Google's own open-source Cloud Code client: `packages/core/src/code_assist/types.ts` and `packages/core/src/utils/googleQuotaErrors.ts`, fetched with `gh api` into `/scratch/tmp/john/agr-research-20261121/gcli/`.

How claims are marked:

- **verified**: I read it in the source cited.
- **unverified**: inferred, or it needs a live Antigravity answer to confirm.

No live Antigravity call was made.

Already known, so not repeated as findings:

- The stale built-in model list.
- The combined weekly pool, pace and attribution gap.
- The iOS app.
- The bare verify-link log.
- The real-UA requirement for the Gemini group.

Where this research adds evidence to one of these, it says so.

---

## Should build (ranked)

### 1. Make an account's own model list authoritative, refresh it, and fail over on a 404
**What**
- Once an Antigravity account has reported its models, route only models in that list (plus explicit `extra-models`). The built-in list is used only before the first report.
- Re-fetch the list every few hours, not once per process.
- On a 404 "Requested entity was not found" from Antigravity, fail over to another account, and drop that model from this account until the next refresh.

**Why**
- Today `resolve_with` serves a model when the built-in list **or** the discovered list has it (`src/accounts.rs:891-893`). `public_models` also merges both (`src/accounts.rs:941-951`). So `claude-*-max`, `gemini-3.1-pro-low`, `gemini-pro-agent` and `gemini-3.7/3.6-flash-high` stay routable on an account that does not offer them.
- A 404 is answered to the client at once: `400 | 404 | 413 | 422 | 504` sets an account error and returns (`src/proxy.rs:2563-2568`). No other account is tried.
- `discovered` is only filled while it is empty:
  - `ensure_ready` (`src/antigravity.rs:311-318`);
  - the poller (`src/quota.rs:906-916`).

  So a model Google adds or removes is not seen until a restart. **verified**
- **unverified** but likely: production's 404s on `gemini-3.1-pro-low` came from `canonical()` (`src/accounts.rs:1921-1962`, candidates at 1954). It tries `-high`, `-medium` and `-low` on a bare name, and it finds the built-in `gemini-3.1-pro-low` for a request like `gemini-3.1-pro`.

**Evidence**
- CPA filters its registry by the account's `fetchAvailableModels` IDs: `filterAntigravityModels`, `sdk/cliproxy/antigravity_models.go:550-560`.
  - Nil means "unknown"; an empty set is authoritative (`sdk/auth/antigravity_models.go:41-45`).
  - It refreshes every 3 h (`registry.ModelsRefreshInterval`, `internal/registry/model_updater.go:17`).
  - It backs off failures at 2, 4, 8, 16 and 30 min. A 403 has its own 10 to 60 min backoff, which a token rotation does not reset (f8e297aa, v8.0.80).
- CPA's current Antigravity registry has no `-max`, `gemini-pro-agent`, `gemini-3.1-pro-low` or `3.7/3.6-flash` IDs (models.json).

**Also**
- Fix the built-in list itself. Keep the Enterprise-only models reachable only through discovery.

**Size / risk**
- M (accounts.rs routing, the poller, the proxy 404 branch, tests).
- Risk: while the list is unknown, an account must keep the built-in list. Otherwise a fresh sign-in serves nothing.

### 2. Handle `TOS_VIOLATION` and `VALIDATION_REQUIRED` as account states, with Google's link
**What**
- Parse Google's 403 `ErrorInfo.reason`.
- **`TOS_VIOLATION`**:
  - take the account out of rotation until the user acts;
  - show the `appeal_url` on the account page and in faults;
  - stop the 60-second model-list polling for it.
- **`VALIDATION_REQUIRED`**:
  - show the verify link and pause the account;
  - re-check with a free `loadCodeAssist` instead of a real request every 10 minutes;
  - clear the pause once `ineligibleTiers` no longer lists `VALIDATION_REQUIRED`.
- Optional, off by default: pause the other Antigravity accounts for a few hours after a first `TOS_VIOLATION`, as CPA allows.

**Why**
- Today a 403 containing "permission_denied" counts as `forbidden_for_good` (`src/proxy.rs:1153-1158`). It lands in `401 | 403 => cool(None, 10 min)` (`src/proxy.rs:2556-2561`). **verified**
- After that 10 minutes, real requests go to a suspended account again. The user sees only Google's message, without the link.
- Google ran a ban wave on 2026-11-06. CPA #7296:
  - the suspension is service-scoped, and the token and model catalog keep working;
  - operators who kept routing sibling credentials "reported the rest of the pool getting banned within minutes" (#7515).

**Evidence**
- The `TOS_VIOLATION` body comes from CPA #7296. Its `metadata` has `appeal_url` and `appeal_url_link_text`.
- CPA's handling: `parseAntigravityTOSViolation`, `applyAntigravityTOSState` and `markAntigravityTOSPause` in `internal/runtime/executor/antigravity_executor_credits.go:367-447` and `:924-982` (commit 5a351d95, v8.0.70).
- Gemini CLI's `classifyValidationRequiredError` (`googleQuotaErrors.ts:147-206`) takes the link from:
  - `google.rpc.Help` → `links[0].url` and `links[0].description`;
  - else `ErrorInfo.metadata.validation_link`, `validation_url` or `validation_learn_more_url`;
  - and accepts only the cloudcode-pa domains.
- `LoadCodeAssistResponse.ineligibleTiers[]` (`types.ts:114-141`) has `reasonCode`, `reasonMessage`, `validationUrl`, `validationUrlLinkText`, `validationLearnMoreUrl` and `validationErrorMessage`.
- CPA only stops the request on `VALIDATION_REQUIRED` and keeps no link (`sdk/cliproxy/auth/conductor_home.go:1480-1533`).
- T3 shows no link either.

**Size / risk**
- S to M.
- Risk: very low for TOS. For validation, the re-check must not hide a real 403 on one model.
- Treat the link as display-only. Allow only `https://` on Google hosts, and never follow it server-side.

### 3. Read quota buckets the way Google labels them: `bucketId`, `disabled`, top-level `buckets`
**What** (in `readings::antigravity`, `src/quota/readings.rs:385-428`)
- **(a)** Skip buckets with `disabled: true`.
- **(b)** Read a top-level `buckets[]` when there are no `groups`.
- **(c)** Use `bucketId` as the limit ID; today the code looks for `id`, which Google doesn't send.
- **(d)** Take each bucket's model scope from its `bucketId` prefix first:

  | Prefix | Scope |
  | --- | --- |
  | `gemini-argon-` | Argon models |
  | `gemini-carbon-` | Carbon models |
  | other `gemini-` | Gemini |
  | `3p-` | Claude & GPT |

  Fall back to the group name only when there is no prefix.

**Why**
- **verified**: Fusebox has none of (a) to (c).
- An unknown group name becomes `Scope::Feature`, which is informational and never routes (`readings.rs:394-401`). If Google sends separate Argon or Carbon groups, a spent Carbon quota would not stop routing Carbon requests.
- A disabled bucket with `remainingFraction: 0` would block a whole family.
- **unverified**: whether this account gets separate Argon or Carbon buckets, or disabled ones. T3 expects both.

**Evidence**
- The T3 binary (module `antigravityUsageLimits.ts`) uses this schema:
  - `Bucket = {remainingFraction?, resetTime?, bucketId (required), window?, displayName?, description?, disabled?}`;
  - `Summary = {groups?: [{displayName?, buckets?}], buckets?}`.
- It skips `disabled`, and labels families by the `bucketId` prefixes above.
- `antigravityWindowLimitsModel` says "Each Antigravity quota blocks only the models it meters": argon, carbon, other-gemini and shared each block only their own models.

**Size / risk**
- S (reader plus tests). The UI's `groupWindows` (`ui/app.js:1038-1057`) already copes with extra groups.
- Risk: low.
- Also, the "Gemini Models" group's `gemini-` prefix (`readings.rs:398`) would then also cover Carbon and Argon. Exclude them only once their own buckets are seen.

### 4. Antigravity in the combined weekly pool, pace warnings and attribution (known gap; how to do it safely)
**What**
- Let `pools.rs` and the attribution and outlook code take an Antigravity account's family-scoped limits as members:
  - the Gemini group's 5h and weekly as the account's own windows;
  - Claude & GPT as a model row.

**Don't convert them into `Window`s.**
- **Why not**: a `Window` with `model: None` applies to **every** model. `QuotaState::exhausted` uses `live(model)` (`src/quota.rs:293-295`, `Window.model` at `:57-59`). A spent Gemini group would then pause Claude and GPT on that account too.
- The UI does this conversion safely only because it keeps `covers` (`ui/app.js:1053`, `1062-1067`). The server has no such field.

**Evidence**
- `pools::Member.windows: &[Window]` (`src/pools.rs:24-36`).

**Size / risk**
- M. Risk: medium, because routing and forecasting share these types. Add a test that a spent Gemini group leaves Claude routable.
- A side benefit: the iOS app gets correct numbers without an app update, if it reads the pool rows.

### 5. Show the Google AI credit balance (read-only)
**What**
- From the same 6-hourly `loadCodeAssist` (`antigravity::load_tier`, `src/antigravity.rs:228-248`), read `paidTier.availableCredits[]` where `creditType == "GOOGLE_ONE_AI"`: `creditAmount` and `minimumCreditAmountForUsage`.
- Show it on the account page, as the Codex credits are shown.

**Why**
- An Ultra plan comes with AI credits. Fusebox already makes the call and drops the field. **verified**
- No spending is involved.

**Evidence**
- CPA `antigravity_executor_credits.go:669-702` (fields above).
- Gemini CLI `types.ts:44-104`: `GeminiUserTier.availableCredits`, and responses carry `consumedCredits` and `remainingCredits`.

**Size / risk**
- S. Risk: low. Amounts are strings (int64 in JSON); keep them as credits, not dollars (docs/dashboard.md already says so for others).

### 6. Send `project` in `fetchAvailableModels`
**What**
- Send `{"project": <id>}` instead of `{}` (`src/antigravity.rs:322-330`).

**Why**
- CPA (`sdk/auth/antigravity_models.go:96-98`) and T3 both send the account's project.
- **unverified**: whether the catalog differs without it. The fact that a project-less answer drops Enterprise or Ultra models is a guess.

**Size / risk**
- XS. Risk: none known.

---

## Maybe (ranked)

### 7. Per-account `request.sessionId`
- Today it is a hash of the first user text only (`src/antigravity.rs:353-369`). So:
  - two Google accounts send the same session ID for the same conversation after a failover;
  - unrelated chats with the same opening text share one.
- CPA 3a24d03c (#7706, v8.0.80) mixes in the credential.
- Mix the account ID (and Fusebox's own session key when there is one) into the hash. XS.
- The anti-correlation benefit is **unverified**. It is cheap given the ban wave.

### 8. Opt-in overflow to AI credits when the Claude & GPT quota is spent
- CPA retries with `"enabledCreditTypes": ["GOOGLE_ONE_AI"]` in the envelope:
  - `injectEnabledCreditTypesDoc`, `antigravity_executor_credits.go:211-217`;
  - for Claude models only (`sdk/cliproxy/auth/antigravity_credits.go:116`);
  - with a 429 `INSUFFICIENT_G1_CREDITS_BALANCE` marking the credits empty.
- This spends credits the user may want for other Google products. Off by default, per account.
- M. Needs item 5 first.

### 9. Treat a zero-output abnormal finish as a retryable failure without a pause
- An example is `finishReason: MALFORMED_FUNCTION_CALL` with no content.
- CPA 7e82d0d1 (#7571) turns it into a 502 that rotates accounts but cools none.
- **unverified** what Fusebox does now. `finish_of` maps unknown reasons (`src/formats/gemini.rs:393`). The stream bootstrap (`src/proxy.rs:2886`) probably answers an empty stop.
- S, after a test confirms the current behaviour.

### 10. Hide the placeholder reset of an untouched window
- T3 `summaryResetTime` drops `resetTime` when `remainingFraction == 1`, there is no description, and the reset is about now plus the window length.
- Fusebox shows "resets in 5h" for a window that hasn't started. That also skews the pace mark.
- XS in `readings.rs`.

### 11. Check that a spent bucket is really read as spent
- If Google omits `remainingFraction` at 0 (a proto3 default), `readings.rs:416` skips the bucket and the group looks fine.
- T3 and CPA also skip a missing value, so it may never happen. **unverified**
- Check the raw answer the first time a group runs out, before changing anything.

### 12. Back off the Antigravity quota check on 429
- The Claude and Codex usage checks go through `usage_answered` and honour Retry-After (`src/quota.rs:237`, `645-648`).
- The Antigravity branch calls `error_for_status` directly (`src/quota.rs:675-689`), so it retries every minute.
- XS.

### 13. Send `thinkingLevel` for Claude 5.5 on Antigravity
- `thinking_config` sends `thinkingBudget` to Claude models (`src/formats/gemini.rs:213-241`).
- The native `agy` client sends `thinkingLevel` for these models, which report `supportsAdaptiveThinking` (CPA #7133, b63322b9).
- Only translated requests with an explicit effort are affected. The ID's own `-high/-medium/-low` suffix still sets the effort.
- The benefit is **unverified**. S.

### 14. Small identity details
- Token refresh goes out with reqwest's default headers. Real Antigravity sends `Go-http-client/2.0` over HTTP/2 (CPA 0861f9e1, #7700).
- The Cloud Code connections advertise ALPN `http/1.1` (`src/tls.rs:70`). CPA sends no ALPN on Cloud Code, "matching the native client" (d0c90bb5).
- Both XS. The ban risk from either is **unverified**.

### 15. Cool the whole quota group on `QUOTA_EXHAUSTED`
- Today only the requested model is paused (`exhaust` → `quota_cooldowns[model]`, `src/accounts.rs:1034-1124`).
- Other models in the same group are tried and refused until the next quota reading.
- One wasted request per model. S.

---

## Don't

- **CPA's reasoning-replay cache.** It stores Gemini thought signatures server-side and re-injects them for clients that drop them (`antigravity_reasoning_replay.go`, ~1,500 lines plus its cache). Fusebox already passes real signatures through and uses `skip_thought_signature_validator` otherwise (`src/formats/gemini.rs:11-12`, `305-313`). Large, with little gain.
- **The Claude signature structure check** (`internal/signature/claude_antigravity_validation.go`). It is structural only, and CPA itself says it proves nothing about replay compatibility.
- **Daily → prod → sandbox base-URL fallback.** CPA removed cross-tier fallback ("selects one request endpoint without cross-tier fallback", `antigravity_executor_request.go:649-656`). Prod returns a placeholder quota (memory note 2026-11-21). Fusebox's daily-only inference matches.
- **The `aicode` OAuth scope.** Already decided against on 2026-11-03 (action list line 12). Nothing new.
- **Sensitive-word obfuscation in system prompts** (CPA `getSensitiveWordMatcher`). It rewrites user content to slip past Google's filters.
- **Emulating CPA's management API (`/v0/management/api-call`, `auth-files`) for T3's hub reader.** T3 can read Antigravity usage through a CPA hub, but Fusebox doesn't implement that API (docs/install.md:74), and the dashboard already shows the meters.
- **Forcing `streamGenerateContent` for non-stream Claude requests,** as CPA's `executeClaudeNonStream` does.
  - Fusebox already streams every translated request upstream (`src/proxy.rs:1925`); only native Gemini non-stream calls use `generateContent`.
  - No failure has been seen. Revisit only if one is.
- **Treating `MODEL_CAPACITY_EXHAUSTED` or "exhausted your capacity" specially.** These come as 503 or as 429 with `RATE_LIMIT_EXCEEDED`. Fusebox's `soft_failure` already retries them briefly, then pauses for 15 s (`src/proxy.rs:680-715`, `2487-2522`), close to Gemini CLI's rule (`googleQuotaErrors.ts:318-330`, `368-378`).

---

## Smaller notes

- **CPA's changelist table** (`internal/misc/antigravity_version.go`) now has 2.23.0, 2.24.0, 2.29.0, 2.32.1, 2.32.2 and 2.33.0, the same as Fusebox's newest. Fusebox reads new releases itself, so nothing is needed.
- **T3's quota check** still claims `antigravity/hub/2.24.0 … cl=963137146`. CPA's 2.24.0 number is 1000335682.
  - T3 tries daily then prod for personal sign-ins, and prod then daily for business sign-ins.
  - It falls back to per-model `quotaInfo` / `weeklyQuotaInfo` from `fetchAvailableModels` when the summary is empty ("older accounts").
  - It reads no plan, tier, credits or verify link. Fusebox already does more there.
- **T3 runs chats through Google's own `agy acp` binary.** So its request behaviour is Google's, and it is no model for Fusebox's proxy path.
- **CPA's Antigravity registry uses one ID per family** (`claude-opus-5-5-high`, `gemini-4-neon-xhigh`, `gemini-4-carbon-high`) and sets the effort with `thinkingLevel`.
  - It also lists `gemini-3.8-flash-tiered`.
  - Fusebox lists each effort as its own ID. Which spelling Google lists in `fetchAvailableModels` today is for item 1's live check to settle.
- **Gemini CLI has `LoadCodeAssistMode` `HEALTH_CHECK`** (`types.ts:72-75`). It would suit item 2's re-check, if the endpoint accepts it for Antigravity (**unverified**).
