# Fork Notes

Rules specific to the `kurerere/clash-verge-rev` fork. `AGENTS.md` stays the
upstream contract; this file holds only what differs here.

## Intent

- Upstream `dev` plus a small set of local features (encrypted
  subscriptions), shipped from this fork's own releases with its own
  auto-update channel. It fully replaces the official app on the maintainer's
  machines.
- Features here don't need a pre-existing issue; the maintainer waived
  issue-first for this fork.
- Out of scope: upstream's release process (version PRs, winget, Telegram,
  SignPath, Linux, macOS x64, fixed-WebView2 builds).

## Constraints

- Keep the upstream app identifier. Installs share the official data directory
  on purpose so existing configs carry over; changing it orphans user data.
  `fork-release.yml` enforces this together with the updater endpoints.
- Release tags are `vX.Y.Z-kure.N`, where `X.Y.Z` is the version on upstream
  `dev`. The updater only offers strictly higher semver, so `N` must increase,
  and restarts at 1 when upstream bumps `X.Y.Z`.
- Releases must be published as non-prerelease Latest: the endpoint is
  `releases/latest/download/latest.json`.
- The updater pubkey in `tauri.conf.json` belongs to this fork's signing key.
  The private key is stored only in the repo secrets `TAURI_PRIVATE_KEY` /
  `TAURI_KEY_PASSWORD` and in the maintainer's backup (`~/.tauri/clash-verge-rev.key`).
  If the key is lost, installed copies can no longer verify updates and must
  be reinstalled manually. Never rotate it casually.
- Background subscription updates never prompt for a password; only
  user-initiated imports and updates do.

## Decisions

- Subscription passwords are stored in plaintext in `profiles.yaml`, matching
  FlClash. A system keychain was rejected because it adds a dependency and
  per-platform handling for a single-user fork. Backups therefore contain the
  password.
- Deep-link imports of encrypted subscriptions only show a notice. Rejected:
  prompting from the deep-link path, which has no owning UI flow.
- The fork releases through its own `fork-release.yml`, not upstream's
  `release.yml`. Upstream's pipeline requires signed bot version commits,
  approval PRs and secrets the fork doesn't have.
- Upstream release and publishing workflows are disabled in the fork's GitHub
  Actions settings, not in code, to keep upstream files merge-clean. After
  merging upstream, check `gh workflow list` for newly added publishing
  workflows and disable them too.

## Lessons

- `pnpm install` rewrites `pnpm-lock.yaml` with unrelated `@pnpm/exe` entries,
  and the pre-push contract check then rejects the push. Restore the file
  before pushing.
- On Windows, Rust tests need `--features clippy`; without it the test binary
  fails with `STATUS_ENTRYPOINT_NOT_FOUND`.

## Pointers

- Automatic syncing: `sync-upstream.yml` checks upstream `dev` every three days
  at 10:17 UTC+8, anchored to 2026-11-15 (next scheduled date: 2026-11-18).
  It uses a daily cadence gate rather than a day-of-month cron step so the
  interval does not reset at month boundaries. GitHub may delay scheduled runs;
  `workflow_dispatch` on `dev` bypasses the cadence gate.
- Set the repository Actions secret `UPSTREAM_SYNC_TOKEN` to a fine-grained
  PAT restricted to this repository, with Contents and Workflows read/write.
  Keep Fork Release enabled. The PAT is necessary to sync upstream workflow
  changes and trigger Fork Release from the tag push; `GITHUB_TOKEN` cannot do
  both. Renew the PAT before it expires.
- A successful automatic merge atomically pushes `dev` and an annotated
  `vX.Y.Z-kure.N` tag. `N` is one greater than all existing tags for the upstream
  base version, or 1 for a new base. No upstream changes means no tag or build.
  Conflicts, updater identity changes, changes to Fork Release, or newly added
  upstream workflows stop the sync without pushing anything. Review these
  manually, including disabling new upstream publishing workflows, then merge
  and release using the manual procedure below. Failed builds can be rerun from
  the Fork Release run; a sync rerun does not tag an already-merged upstream.

- Syncing upstream: remote `upstream` is fetch-only; `git fetch upstream && git merge upstream/dev`.
  On conflicts, keep the fork's values for the updater config, the release
  link in `update-viewer.tsx` and the version regex in `scripts/release-version.mjs`.
- Releasing: push a `vX.Y.Z-kure.N` annotated tag; `fork-release.yml` builds
  and publishes it.
