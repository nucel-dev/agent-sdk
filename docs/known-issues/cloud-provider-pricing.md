# Known issue: Bedrock/Vertex budget guard silently fails on current model ids

**Status:** open — blocks publishing `nucel-agent-bedrock` and `nucel-agent-vertex`
**Severity:** high (unbounded spend, no error)
**Affects:** `nucel-agent-bedrock` 0.1.2, `nucel-agent-vertex` 0.1.1 (neither has
ever been published to crates.io)
**Filed:** 2026-07-26

Both cloud-provider crates are marked `publish = false` in their manifests until
this is fixed. Do not remove that marker while this document says "open".

---

## 1. The budget guard never trips on current models

`budget_usd` on `SpawnConfig` is enforced by comparing accumulated cost against
the budget. Accumulated cost comes from a hardcoded per-million-token price
table looked up **by substring match on the model id**:

- `crates/bedrock/src/pricing.rs` → `lookup()`, consumed at
  `crates/bedrock/src/lib.rs:208`
- `crates/vertex/src/pricing.rs` → `lookup()`, consumed at
  `crates/vertex/src/lib.rs:271`

Both tables only know the Claude 3.x and Claude 4.x families:

| Table branch | Matches |
|---|---|
| `claude-opus-4-7`, `claude-opus-4` | Opus 4.x only |
| `claude-sonnet-4`, `claude-3-5-sonnet` | Sonnet 4.x / 3.5 only |
| `claude-haiku`, `claude-3-5-haiku` | Haiku 4.x / 3.5 |
| `claude-3-sonnet`, `claude-3-haiku` | Claude 3 |

**`claude-opus-5` and `claude-sonnet-5` match none of these branches.** The
string `"claude-opus-5"` does not contain `"claude-opus-4"`. So for any session
on a current model:

1. `lookup()` returns `None`.
2. The caller treats an unknown model as zero cost.
3. `AgentCost.total_usd` stays at `0.00` for the whole session.
4. The `budget_usd` comparison is always `0.00 < budget` — **the guard never
   trips, no matter how much the session actually spends.**

There is no error, no warning, and no visible symptom until the AWS or GCP
invoice arrives. A caller who sets `budget_usd: Some(5.00)` as a safety rail
gets no rail at all.

Token counts (`input_tokens`, `output_tokens`, cache tokens) are captured
correctly on `AgentCost` in both crates — only the USD conversion is broken.

## 2. Vertex's Opus price is wrong even on models it does match

`crates/vertex/src/pricing.rs` documents itself as:

> Source: Anthropic's Vertex documentation (Vertex matches direct Anthropic API
> list price 1:1 for Claude SKUs).

and then encodes Opus at **$15 input / $75 output per MTok**. Anthropic's
first-party list price for the Opus tier (Opus 5, 4.8, 4.7, 4.6) is **$5 / $25
per MTok** — a 3× overstatement. The Haiku row ($0.80 / $4.00) is likewise below
the current Haiku 4.5 first-party price ($1.00 / $5.00).

Separately, the doc comment's premise needs re-checking rather than
re-encoding: Claude on Vertex AI and on Amazon Bedrock are **partner-operated
with their own pricing**, so "matches Anthropic list price 1:1" cannot be
assumed and must be confirmed against the current
[Vertex AI](https://cloud.google.com/vertex-ai/generative-ai/pricing#claude-models)
and [Bedrock](https://aws.amazon.com/bedrock/pricing/) pricing pages.

## 3. Bedrock ships a "placeholder" comment in released code

`crates/bedrock/src/pricing.rs` carries, in the shipped `lookup()` body:

```rust
// Claude Opus 4.7 (placeholder pricing — operator to verify)
```

Placeholder numbers in a code path that gates spend are not acceptable in a
published crate.

---

## Knock-on effect: `nucel-agent-sdk` cannot be published either

`crates/unified/Cargo.toml` declares the cloud crates as optional dependencies
with a `version` key:

```toml
nucel-agent-bedrock = { path = "../bedrock", version = "0.1.2", optional = true }
nucel-agent-vertex  = { path = "../vertex",  version = "0.1.1", optional = true }
```

At publish time cargo strips the `path` and resolves the `version` against
crates.io — **for optional dependencies too**. Since neither crate has ever been
published, `cargo publish -p nucel-agent-sdk` fails:

```text
error: failed to prepare local package for uploading
Caused by:
  no matching package named `nucel-agent-bedrock` found
  location searched: crates.io index
  required by package `nucel-agent-sdk v0.2.4`
```

This is **pre-existing**, not a consequence of the `publish = false` marker —
the marker changes nothing about registry resolution. It was missed by earlier
release checks because `cargo package --workspace --no-verify` does not resolve
dependencies against the registry.

**It does not block the fixes from reaching consumers.** The published
`nucel-agent-sdk` 0.2.0 requires `^0.2.0` on `nucel-agent-core`,
`-claude-code`, `-codex`, and `-opencode`, so anyone depending on
`nucel-agent-sdk = "0.2"` picks up the new provider versions as soon as those
four are published. The 0.2.4 umbrella bump carries no code of its own — it is
a re-export version bump.

Two ways out, whenever the umbrella needs republishing:

1. **Fix the pricing defect and publish the cloud crates.** Preferred — it
   keeps `--features bedrock` / `--features vertex` working for umbrella users.
2. **Drop the `bedrock` / `vertex` / `all-providers` features from
   `crates/unified`.** Cheaper, and breaks no published consumer (those
   features have never shipped on crates.io — they were added after 0.2.0).
   Users would depend on the cloud crates by path or git instead.

`.github/workflows/publish.yml` guards this with a pre-flight check, so a
release tag fails *before* publishing anything rather than half-way through.

## What a fix must do

1. **Refuse or warn on `None`, never silently charge $0.00.** An unknown model
   id must not degrade into "this session is free". The minimum acceptable
   behaviour is a loud `tracing::warn!` plus a flag on `AgentCost` marking the
   total as unpriced; the safer behaviour, when `budget_usd` is set, is to fail
   the spawn with a clear error — a caller who asked for a budget has stated
   that unbounded spend is unacceptable, and an unpriceable model cannot honour
   that request.
2. **Replace substring matching with something that fails loudly on new
   families.** Substring matching is what turned "we don't know this model" into
   "this model is free"; it will silently re-break on `claude-opus-6`.
3. **Populate the tables from current partner list prices**, confirmed against
   the AWS and GCP pricing pages by a human — not copied from this document.
   Include Opus 5 and Sonnet 5.
4. **Delete the "placeholder pricing" comment** once the numbers are real, and
   correct or remove the "matches Anthropic list price 1:1" claim in the Vertex
   module docs.
5. **Add regression tests** asserting that the current model ids used by the
   `DEFAULT_MODEL` constants — and at least one deliberately unknown id —
   produce the intended behaviour, so the next model generation trips a test
   instead of a bill.

Only then flip `publish = false` back to publishable in
`crates/bedrock/Cargo.toml` and `crates/vertex/Cargo.toml`, and bump both
crates.
