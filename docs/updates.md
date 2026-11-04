# Over-the-air updates

The Brick updates itself over Wi-Fi, without your computer or any account:

```
git push (branch mortar)
  └─ GitHub Actions (.github/workflows/build.yml)
       ├─ settings: can this publish? does everything evaluate? is it all allowed public?
       ├─ VM tests (x86 runner with KVM), the other checks (arm64 runner)
       ├─ nix build brick-bare, brick-full
       ├─ what cache.nixos.org doesn't have, to the R2 bucket   store paths, signed with your Nix key
       ├─ channels/testing/<config>.json                        manifest, signed, written last
       └─ images/testing/<config>-latest.img.zst                SD images, with sha256
Actions > Promote to release (.github/workflows/promote.yml, by hand)
       ├─ channels/mortar/<config>.json                         same system, manifest signed for this channel
       └─ images/mortar/<config>-latest.img.zst                 the image of that commit
Cloudflare Worker (cloudflare/worker.js)                        read-only HTTPS in front of the bucket
Brick (mortar-update: Settings > Updates > Check now, or daily)
  ├─ curl <baseUrl>/channels/<channel>/<config>.json            checks signature, channel, configuration, age
  ├─ nix-store --realise <toplevel>                             from <baseUrl> and cache.nixos.org; Nix checks
  │                                                             every signature against the Brick's own keys
  ├─ system profile + switch-to-configuration boot              a new boot entry; the old ones stay
  └─ toast "Update ready — restart to apply"
```

No credentials are involved in reading. The connection isn't what's trusted either: a
Brick believes a manifest because of its signature, and a store path because of its
signature, whoever served them.

**Not exercised yet.** All of this has run in VMs (`checks.*.mortar-update`, against
`cloudflare/worker.js` and a stand-in for R2), with the worker's own tests. The Worker
has not been deployed, nothing has been published to the public URL, and no Brick has
updated from it. Take this page as how it's built until that has happened.

`mortar publish` does the CI part from your computer when CI is not available.

## What may be published

The repository is public. What CI builds and hosts follows this, and CI fails the run
if it stops being true (`mortar.public`, `tests/public-policy.nix`,
`scripts/check-public-closure.sh`):

| | Public image and cache? |
|---|---|
| Mortar and free software | Yes |
| TrimUI's files for the Brick: GPU userspace (`pvr-ddk-userspace`), Wi-Fi, Bluetooth and GPU firmware (`trimui-brick-firmware`), SCP in the boot image (`trimui-brick-scp`, `u-boot-trimui-brick-mainline`) | Yes, by the project's decision (below) |
| Claude Code | **No.** Commercial software; `nixosModules.pack-ai` for your own system |
| FBNeo, Genesis Plus GX, Snes9x 2005 | Yes. Their licences allow free distribution, not sale, and FBNeo's rules out asking for donations, so Mortar does neither. Their licences ship in `/etc/mortar-play/licenses`. `mortar.play.nonCommercialCores = false` builds without them |
| PICO-8 and game data | **No.** Never in the repository, CI or a cache. They stay in profiles of their own on the Brick (`mortar pico8`), and updates leave those alone |

**The decision on TrimUI's files.** They are proprietary, and we have no licence
granting redistribution. TrimUI publishes them itself in its public firmware
releases, for this device, and the project has decided on that basis to host them
in the public bare image and cache. That is this project's call about its own risk.
It gives nobody rights in the files, it isn't TrimUI's permission, and it would be
reversed if TrimUI objected. They are never committed to this repository. If you
distribute them yourself, make your own decision.

A flake of your own with Claude Code can use all of this with a **private** bucket:
[A flake of your own](#a-flake-of-your-own). `mortar publish` signs and pushes what you
tell it to, and applies the checks by name only with `--upstream`. Its bucket must stay
private (no r2.dev URL, no custom domain, no Worker), and it must not be this one.

## Channels

`channels/<channel>/<config>.json`. Each push goes to `testing`. A Brick follows the
`mortar.update.channel` of the system it runs, and for the published systems that is one
line: `channel` in `configurations/public.nix`, `testing` until the owner promotes a
release.

A push to `mortar` only runs the settings and VM tests, and skips those if it changes
nothing but documentation. Tags are what build and publish. Create them by hand, when you
mean to:

| Tag | What it does |
|---|---|
| `v0.1.0-rc.1` | Builds and publishes to `testing` |
| `v0.1.0` | Promotes that commit from `testing` to the release channel; builds nothing |

To release:

1. Set `channel = "mortar"` in `configurations/public.nix` and commit. This is a step of its
   own because a Brick keeps following what its system says: a system that says `testing`
   would take its Brick back to `testing` the moment it was promoted, and
   `promote.yml` refuses one. Bricks that are on `testing` take this system when they
   check, and from then on follow the release channel.
2. Tag that commit `v0.1.0-rc.1`, push the tag, and try the build from `testing` on a Brick.
   Fixing something means another commit and `v0.1.0-rc.2`.
3. Within 3 days (step 3 of the setup below), tag the **same commit** `v0.1.0` and push it.
   It needs no build: the closure is in the bucket. The manifest is signed again under the
   new tag, as the channel is part of what's signed, and that commit's image is copied to
   `images/mortar/`. **Actions > Promote to release > Run workflow** does the same by hand,
   and `mortar promote --from testing --rev COMMIT` from a computer. To require a reviewer,
   add one to the `release` environment (Settings > Environments).
4. Point the README and the Worker's index page (`IMAGE` in `cloudflare/worker.js`) at
   `images/mortar/`.

A Brick's floor is the newest manifest it has taken, on any channel. So one that has taken
a build from `testing` refuses an older one promoted afterwards as a downgrade, until
something newer is released. Promoting gives the manifest a new date, so this needs a
Brick to have followed `testing` past what's being promoted and then changed channel.

## What is where

| Thing | Where | Secret? |
|---|---|---|
| Public URL, channel, public signing key | `configurations/public.nix` | no |
| Account the bucket is in | GitHub secret `R2_ACCOUNT_ID`; `CLOUDFLARE_ACCOUNT_ID` when deploying the Worker | not in the repository |
| Signing key (secret half) | `~/.config/mortar/signing-key` on your computer; GitHub secret `NIX_SIGNING_KEY` | yes |
| R2 write token | GitHub secrets `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`; `~/.config/mortar/r2-write` on your computer | yes |
| A private bucket's read-only token | that Brick: `/var/lib/mortar-update/credentials` (root, 0600, ext4 root fs). A published Brick has none | yes |

No secret is ever in the repository or the Nix store.

## What's in the bucket

| Key | What | Worker's `Cache-Control` |
|---|---|---|
| `nix-cache-info` | `WantMassQuery: 1`, `Priority: 41`: right after cache.nixos.org (40), so that's asked first and this cache isn't asked for what it doesn't hold. Written by `mortar publish`; Nix alone only ever writes `StoreDir` | 1 hour |
| `<hash>.narinfo`, `nar/…` | store paths that aren't on cache.nixos.org | immutable |
| `channels/<channel>/<config>.json` | the manifest, and `channels/<channel>/<config>/…` a copy of each | 60 s |
| `images/<channel>/<config>-latest.img.zst`, `.sha256` | the name that stays | 60 s |
| `images/<channel>/v/<config>-<rev>.img.zst`, `.sha256` | each commit's | immutable |

Nothing else in the bucket is served, there is no listing, and nothing can be written
through the Worker. What a first publish puts there, measured when this was written (a later
one uploads only the store paths that changed):

| | Closure | On cache.nixos.org | Uploaded | SD image |
|---|---|---|---|---|
| `brick-bare` | 2.85 GiB, 919 paths | 751 paths | 168 paths, 95 MiB, 30 MiB with zstd | 867 MiB |
| `brick-full` | 3.38 GiB, 986 paths | 796 paths | 190 paths, 438 MiB, 110 MiB with zstd | 1003 MiB |

A Brick downloads the rest of an update from cache.nixos.org. If cache.nixos.org ever
drops an old path a published system needs, an update to that system fails: the fix is to
publish without `--upstream`, which pushes everything.

`nix copy --to` can't do this by itself: it refuses a path whose references aren't in the
destination, and `nix copy --from` wants the whole closure in one place. So publishing
copies to a local directory seeded with the narinfos of what's elsewhere, then uploads only
what Nix added (`push_new_paths` in `pkgs/mortar-cli/src/publish.rs`); and a Brick
substitutes, with both caches as substituters, instead of copying. Paths from the upstream
are checked against the upstream's key, which counts for no manifest.

## Runners and cost

On a public repository GitHub's standard hosted runners are free and unlimited, arm64
included: `ubuntu-24.04-arm` has 4 vCPUs, 16 GB RAM and 14 GB of SSD (the workflow frees
space first). A job can run for 6 hours. (A fork set to private pays per minute against
its plan and gets 2 vCPUs and 8 GB.) The first build compiles the kernel and the Rust
apps; later builds take unchanged paths from the public cache and only rebuild what
changed. The disk is the tight part: after publishing, the workflow collects garbage
(keeping both systems) before each image.

Sources: [GitHub-hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners),
[Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions),
[Actions limits](https://docs.github.com/en/actions/reference/limits).

**Where each check runs.** `mortar-update` and `usb-pairing` are NixOS VM tests and
need KVM, so they run on the x86 runner as `checks.x86_64-linux.*`, after opening up
`/dev/kvm`. GitHub's documentation doesn't say the arm64 runners have it, and the
workflow doesn't rely on it. They also run on any aarch64 machine with KVM, as
`checks.aarch64-linux.*`, such as a Mac's linux-builder. All the other checks run on
the arm64 runner. A pull request runs the evaluation and the VM tests, without secrets;
the arm64 build is for pushes and manual runs.

Fallbacks (Actions > Build and publish > Run workflow > runner):

- `x86-qemu`: an x86 runner building aarch64 under QEMU. Works, but many
  times slower; a full rebuild can hit the 6-hour job limit.
- `self-hosted`: your own runner with the label `mortar` (for example on a
  Mac, where Nix builds through the linux-builder).
- No CI at all: `nix run .#mortar -- publish` on your computer.

R2 storage is $0.015/GB-month after a free 10 GB-month; downloads (egress)
are free. Each release candidate adds about 1.9 GB of images, kept 3 days, which is what to watch; see
[Cleaning the bucket](#cleaning-the-bucket). A request through the Worker counts against
the Workers plan's and R2's request allowances, and the Worker keeps no cache of its own.

## One-time setup

This is the owner's, for the public cache. Run these from this repository; `wrangler` is
`npx wrangler`, logged in with `npx wrangler login`. A flake of your own that isn't
public wants [A flake of your own](#a-flake-of-your-own) instead.

### 1. Bucket and Worker (Cloudflare)

```sh
export CLOUDFLARE_ACCOUNT_ID=<ACCOUNT_ID>     # R2 > Overview; keep it out of the repository
npx wrangler r2 bucket create mortar-cache
cd cloudflare && npx wrangler deploy          # says https://mortar.<subdomain>.workers.dev
```

The subdomain has to be `angelfire` (Workers > Subdomain), or `baseUrl` in
`configurations/public.nix` is wrong. Leave the bucket's public access off (no r2.dev URL,
no custom domain): the Worker is the only way in.

Check it: `curl -s https://mortar.angelfire.workers.dev/` is the index page,
`curl -sI https://mortar.angelfire.workers.dev/channels/` is a 404, and a `curl -X PUT` of
anything is a 405.

### 2. Write token (Cloudflare dashboard, by hand)

Wrangler can't make S3 tokens. **R2 > Overview > Account Details > API Tokens > Manage >
Create Account API token**: name `mortar-ci`, permission **Object Read & Write**, applied
to the bucket `mortar-cache` only. It shows an **Access Key ID** and a **Secret Access Key**
once. No token is needed to read, and the Worker gets at the bucket through its binding.

### 3. Old testing images expire

Every release candidate adds an image of each configuration under `images/testing/v/`. Promoting copies
the one it needs, so these can go soon: promote within 3 days of the build, or run the
workflow again first, or the release has no new image. The name that stays is outside the
prefix.
(`--help` for the flags; the dashboard's bucket Settings has the same.)

```sh
npx wrangler r2 bucket lifecycle add mortar-cache testing-images images/testing/v/ --expire-days 3
```

### 4. GitHub

```sh
gh secret set NIX_SIGNING_KEY -R gopherbone/mortar < ~/.config/mortar/signing-key
gh secret set R2_ACCESS_KEY_ID -R gopherbone/mortar        # mortar-ci Access Key ID
gh secret set R2_SECRET_ACCESS_KEY -R gopherbone/mortar    # mortar-ci Secret Access Key
gh secret set R2_ACCOUNT_ID -R gopherbone/mortar           # the account ID
```

(`gh secret set NAME` without `<` asks for the value.) The `settings` step fails at once if
one is missing, or if `NIX_SIGNING_KEY` isn't the secret half of `publicKey` in
`configurations/public.nix`. Check that Actions is enabled for the repository, and make
the `release` environment (Settings > Environments) that `promote.yml` runs in.

CI on push stays off until you turn it on, so pushes don't spend
minutes or fail before the secrets exist:

```sh
gh variable set MORTAR_CI -R gopherbone/mortar --body true
```

### 5. Your Mac as a fallback publisher (optional)

```sh
cat > ~/.config/mortar/r2-write <<'EOF'
access_key_id = "<mortar-ci Access Key ID>"
secret_access_key = "<mortar-ci Secret Access Key>"
EOF
chmod 600 ~/.config/mortar/r2-write
ENDPOINT=https://<ACCOUNT_ID>.r2.cloudflarestorage.com
mortar publish --endpoint $ENDPOINT --bucket mortar-cache --upstream https://cache.nixos.org \
  --channel testing --config brick-bare     # or --dry-run to see the manifest
```

### 6. Try it

Push a commit and watch **Actions > Build and publish**. Then, with a Brick on the image from
`images/testing/`: **MENU > Settings > Updates > A** (Check now). When the toast says
**Update ready — restart to apply**, Settings > Updates says **Restart to update**; press A
twice there (or restart any other way). Afterwards, update the note at the top of this page.

## A flake of your own

A system built on `nixosModules.full`, `nixosModules.core` or `lib.mkSystem` has updates
**off**: nothing to do, and it follows nothing. The settings that follow the public channel
are in `configurations/public.nix`, which is not a flake output. Don't import it: a Brick
following the public channel replaces its whole system with the published one, and anything
of your own is gone from the next boot. The check that a manifest is for this configuration
wouldn't stop this, since `nixosModules.full` calls itself `brick-full` too
(`tests/public-policy.nix` holds all of this in place).

To update a Brick of your own over Wi-Fi, use a bucket of your own, private, and read it
with credentials (S3 mode):

```nix
mortar.update = {
  enable = true;
  bucket = "my-brick-cache";
  endpoint = "https://<ACCOUNT_ID>.r2.cloudflarestorage.com";
  publicKey = "my-brick-1:…=";   # a key of your own: nix key generate-secret --key-name my-brick-1
  configName = "my-brick";       # not brick-bare or brick-full
  channel = "mortar";
};
```

(`baseUrl` stays unset; setting it with a bucket is an evaluation error.) In a public
repository this makes the bucket's name and endpoint public, which isn't a secret but is an
invitation; they aren't credentials. So does keeping them out of the repository, in a
module of your own.

```sh
# Make two tokens: one Object Read & Write, one Object Read only, on that bucket only.
mortar publish --flake . --config brick --key ~/.config/mortar/my-brick-key   # the whole closure
mortar deploy --reboot            # once, over USB, so the Brick has this system
mortar update-creds               # the read-only token; or [update] in mortar.toml below
```

On the card, in `NIXOS_BOOT/mortar.toml`:

```toml
[update]
access_key_id = "<read-only Access Key ID>"
secret_access_key = "<read-only Secret Access Key>"
```

At the next boot the Brick moves the secret to `/var/lib/mortar-update/credentials` (root
only) and blanks it in the file. S3 mode copies with `nix copy --from s3://…`, and checks
only your keys, so the whole closure is in your bucket: `mortar publish` without
`--upstream` pushes all of it.

## On the Brick

- **Settings > Updates**: the status; **A** checks now, or, with an update
  staged, restarts into it (press twice).
- **Settings > Auto-check daily** (`mortar.update.autoCheck`; `[update] auto_check` in
  `mortar.toml`). The daily check (`mortar-update-auto.timer`, also 20 minutes after boot)
  downloads and stages an update by itself; it never restarts the Brick. On the published
  systems it starts **off while they follow `testing`**, which takes every release candidate that passes
  CI with no one having run it on a Brick, and **on once they follow the release channel**,
  which moves only by hand (`configurations/public.nix`). A flake of your own gets on.
- No download while the battery is below 30% and not charging
  (`mortar.update.minBattery`); the row says "Charge to 30% first".
- Offline: "Offline", and the next check tries again. Store paths arrive
  whole or not at all, so an interrupted download resumes where it stopped.
- Space: below 1 GiB free in `/nix/store` unreferenced paths are collected
  first. When an update is staged, system generations beyond the newest 5
  are deleted (`mortar.update.keepGenerations`).
- A manifest is only taken if it is newer than the last one taken, so a
  build you `mortar deploy` from the Mac is not replaced by an older
  published one; the next publish updates it as usual.
- A manifest is refused, with the reason in Settings > Updates, when:
  - it isn't signed by a key in `mortar.update.trustedKeys`, or was changed
    since (every field is covered);
  - it names another channel, Nix system or configuration, so a bare Brick
    doesn't silently become a full one. `mortar-update check
    --allow-config-change` takes it, once;
  - it is older than the last one taken: an old manifest put back in the bucket
    stays out. `mortar-update check --allow-downgrade` takes it. The floor is
    what this Brick has taken or found running from the channel: a fresh card
    has none until its first check, and a system you `mortar deploy` doesn't
    raise it, since it has no publication date.
- Each path of the closure must be signed by those keys or, in HTTPS mode, an upstream's
  (`mortar.update.upstreams`, by default cache.nixos.org's). For that download these replace
  `nix.settings.trusted-public-keys` and the substituters in `nix.conf`, so another key
  trusted on the Brick counts for nothing. In S3 mode there's no upstream, so a path with
  cache.nixos.org's signature alone isn't accepted from the bucket.
- The connection needs the clock to be about right, for TLS: a Brick that has never been
  online since it lost its time says "Check failed" until it has synced.
- On SSH: `mortar-update status`, `mortar-update check`,
  `journalctl -u mortar-update -u mortar-update-auto`.

## Rolling back

Every update is a new system generation; the previous ones stay installed.

- **From the Mac**: `mortar rollback` (switches now), or
  `mortar generations` to look first.
- **On the Brick** (Terminal or SSH): `mortar-update rollback`, then restart:
  the previous generation becomes the boot entry. The same published
  update is not taken again; the next publish is.
- **On the Brick, in Settings**: Settings > Updates > Roll back (press A twice),
  then restart. Same as above.
- **If the new system does not boot**: hold SELECT while powering on, until a
  second buzz. That boots the previous generation, once (tested on a real Brick); then
  roll back as above to make it stick. This needs a card with a U-Boot from after the
  feature was added (it isn't updated with the system: firmware/mainline/README.md). The
  published images have it.
- **If that doesn't help**: the U-Boot menu lists the last 5 generations, but only
  on the UART console. Without one, put the card in a computer, open
  `NIXOS_BOOT/extlinux/extlinux.conf` and change the `DEFAULT` line to the label of
  the previous entry (the `LABEL` lines below it, newest first), then boot and run
  `mortar-update rollback` to make it stick.
- **The channel itself**: publish an older system again, e.g.
  `mortar publish --path /nix/store/…-nixos-system-mortar-… --force` with the arguments
  of [step 5](#5-your-mac-as-a-fallback-publisher-optional) (the `toplevel` of an older
  manifest; `channels/mortar/<config>/` in the bucket keeps a copy of every manifest), or
  revert the commit and push. Publishing gives it a new date, so Bricks take it.

## Cleaning the bucket

Each publish adds only the store paths that changed, and old store paths and manifests are
never deleted. Images are the larger part: `images/testing/v/` expires by the rule of step 3.
When the bucket grows too large, empty it (R2 dashboard > bucket > Settings > delete objects,
or a lifecycle rule) and re-run the workflow: it pushes what the current systems need again.
Bricks are unaffected: they already have the paths they run. Don't expire narinfos alone,
as a publish takes one to mean its path and all it needs are there.

## Rotating secrets

- A private bucket's read token leaked or Brick lost: create a new **Object Read only**
  token, `mortar update-creds` on your Bricks, delete the old token.
- Write token: create a new one, update the two GitHub secrets (and
  `~/.config/mortar/r2-write`), delete the old one.
- Signing key: generate a new one (step 2). A Brick only trusts the keys of the
  system it runs, so first publish a system that has both,
  `mortar.update.trustedKeys = [ "old:…" "new:…" ]`, still signed with the old
  key, and wait for your Bricks to take it (or `mortar deploy` them). Then update
  `NIX_SIGNING_KEY`, set `publicKey` to the new key and drop the old one from
  `trustedKeys`, and empty the bucket so everything is re-signed. A narinfo is served as
  immutable, so give caches a year or re-publish under new paths; Nix on a Brick doesn't
  keep one across checks.

## Formats

`channels/<channel>/<config>.json` (pkgs/mortar-update/src/manifest.rs):

```json
{
  "schema": 1,
  "channel": "mortar",
  "system": "aarch64-linux",
  "config": "brick-full",
  "toplevel": "/nix/store/…-nixos-system-mortar-26.05…",
  "published": 1793525400,
  "date": "2026-11-01T09:30:00Z",
  "rev": "fb4f6aa…",
  "label": "26.05…",
  "changelog": ["USB MIDI: ALSA sequencer in the kernel, loaded at boot; README"],
  "publisher": "github-actions",
  "signature": "mortar-1:base64…"
}
```

`signature` is Ed25519 with the publisher's Nix key, over `mortar-manifest-1\n` and
the manifest as compact JSON without that field, in the field order above.
`mortar sign-manifest --key FILE < manifest.json` makes one by hand. A publisher
that adds a field bumps `schema`, since an older Brick couldn't check it.

The Brick's status, for the shell: `/run/mortar-update/status.json`
(`state`: unconfigured, idle, checking, downloading, installing, ready,
up-to-date, offline, low-battery, error).

The end-to-end test (`nix build .#checks.aarch64-linux.mortar-update`) runs the whole path
in VMs. With Garage as the S3 server: read key cannot write, credentials from the card,
unsigned, tampered, wrong-key, wrong-channel, wrong-configuration and older manifests refused,
unsigned closures and closures signed by a key only `nix.conf` trusts refused, low battery,
publish, download and stage, offline, rollback, testing and promotion, auto-check. Over plain
HTTP with no credentials, through `cloudflare/worker.js` in front of a directory holding what
`mortar publish --upstream` put in the bucket, and a second cache standing in for
cache.nixos.org: the same refusals, a manifest signed by the upstream's key, a closure served
by the upstream under the wrong key, a corrupted NAR, only the upstream's path missing from
the bucket, nothing uploaded on a second publish, each path from its own cache, offline,
rollback, downgrade, and an image published, promoted and downloaded in two ranges.
`checks.*.cloudflare-worker` is the Worker's own tests (`node --test
cloudflare/test/worker.test.mjs`), against a stand-in for R2.
