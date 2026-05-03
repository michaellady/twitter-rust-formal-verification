# Deploy — `twitter-formal-rust`

User-facing backend on Fly.io. The deploy stack is in TCB (see `TCB.md`).

## Architecture

```
PR → verify workflow (gobra/verus/tlc all strict) ──┐
                                                     │ on success
                            ┌────────────────────────┘
                            │
                  build-image-main job
                            │ pushes ghcr.io/.../...@sha256:<digest>
                            │ uploads image-digest.txt artifact
                            ▼
                  deploy workflow (workflow_run trigger)
                            │ downloads image-digest artifact
                            │ flyctl deploy --image @sha256:<digest>
                            │ post-deploy /version digest verification
                            ▼
                  twitter-formal-rust.fly.dev
```

No source rebuild in the deploy job — the deployed image is byte-identical to the verified image. See K3 in the Tier-4 plan for full rationale.

## First-time manual setup

1. Install flyctl: `brew install flyctl` (or `curl -L https://fly.io/install.sh | sh`)
2. `flyctl auth login` — opens browser, sign up or sign in
3. `flyctl apps create twitter-formal-rust` — or pick another name; update `app` in `fly.toml` AND the GitHub `vars.FLY_APP` accordingly
4. `flyctl tokens create deploy --expiry 720h` — copy the token
5. In the GitHub repo settings → Secrets → Actions, add:
   - `FLY_API_TOKEN` = the token from step 4
6. In Settings → Variables → Actions, add:
   - `FLY_APP` = `twitter-formal-rust`
7. `flyctl deploy --remote-only` once manually — to verify the Dockerfile builds, the app boots, and `/healthz` returns 200. After this manual seed deploy, every subsequent push to main triggers an automated deploy via the workflow.

## Health-check endpoint

`GET /healthz` returns `200 ok` once the server has bound the listener.

## Rollback procedure

```sh
flyctl releases list                       # find the previous release version
flyctl releases rollback <version>         # re-pulls the previous image; no rebuild
curl -fsS https://twitter-formal-rust.fly.dev/version | jq .image_digest
                                           # confirm the rolled-back digest
```

The image-digest is baked into `/etc/version.json` inside each image, so `/version` always reports the digest of whatever image is actually running — including after rollback. No env-var sync required.

## Secrets and rotation

| Secret | Where stored | What for | Rotation procedure |
|---|---|---|---|
| `FLY_API_TOKEN` | GitHub repo secret | deploy workflow auth | quarterly: `flyctl tokens create deploy --expiry 720h`, then update the GitHub secret atomically (gh CLI: `gh secret set FLY_API_TOKEN`) |
| `GHCR_TOKEN` (auto) | GitHub default `secrets.GITHUB_TOKEN` | image push to GHCR | no rotation; per-job ephemeral |
| `ADMIN_TOKEN` | Fly secret on the app | snapshot/load-snapshot endpoints (added in Stream 2 Phase 0) | `flyctl secrets set ADMIN_TOKEN=$(openssl rand -hex 32)` simultaneously on both backends so the diffsplitter doesn't lose admin access mid-rotation |
| `UI_COOKIE_HMAC_KEY` | Fly secret on the UI app | demo-login cookie integrity (Stream 1 Phase 2) | with key-versioning (cookies prefixed `kv:1`/`kv:2`); old cookies valid for 7 days after rotation |

| Variable | Where | What for |
|---|---|---|
| `FLY_APP` | GitHub repo variable | the user-facing app name; deploy workflow reads this for the `/version` post-deploy check |

## What persists

**Nothing** across both-backends-down. The verified core is in-memory by design; Fly machine restart resets to seed. When stream 2 phase 1b is live, restart of one backend triggers automatic resync from peer (see `internal/clock/clock.go` resync-hook deliverable). Both-down → seed-only with writes globally blocked until manual `flyctl deploy --image @digest` reset.

## Trust boundary

The deploy stack is in TCB. Specifically:
- `Dockerfile`, `fly.toml`, `.github/workflows/deploy.yml`
- `GET /healthz`, `GET /version` endpoints
- Future: `POST /_admin/snapshot`, `POST /_admin/load-snapshot`, `POST /_admin/begin-resync`, `POST /_admin/mark-live` (Stream 2 Phase 0)
- The image-digest verification gate (mismatch fails the deploy)

Inventoried in `TCB.md` whenever an item is added.
