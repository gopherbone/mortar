# Over-the-air updates

The Brick updates itself over Wi-Fi, without your computer:

```
git push (branch mortar)
  └─ GitHub Actions (.github/workflows/build.yml)
       ├─ settings: can this publish? does everything evaluate? is it all allowed public?
       ├─ VM tests (x86 runner with KVM), the other checks (arm64 runner)
       ├─ nix build brick-bare, brick-full
       ├─ nix copy --to s3://BUCKET (Cloudflare R2)            closures, signed with your Nix key
       └─ channels/testing/<config>.json                       manifest, signed, written last
Actions > Promote to release (.github/workflows/promote.yml, by hand)
       └─ channels/mortar/<config>.json                        same system, manifest signed for this channel
Brick (mortar-update, daily or Settings > Updates > Check now)
  ├─ reads channels/mortar/<config>.json with a read-only key  checks signature, channel, configuration, age
  ├─ nix copy --from s3://BUCKET <toplevel>                    Nix checks every signature against the same keys
  ├─ system profile + switch-to-configuration boot             a new boot entry; the old ones stay
  └─ toast "Update ready — restart to apply"
```

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

A flake of your own with Claude Code or those cores can use all of this with a
**private** bucket: `mortar publish` signs and pushes what you tell it to, and doesn't
apply these checks. Its bucket must stay private (no r2.dev URL, no custom domain).

A Brick reads the bucket with credentials today, public cache or not
(`mortar.update`; step 6): nothing here gives an anonymous Brick updates yet.

## Channels

`channels/<channel>/<config>.json`. Each push goes to `testing`. A Brick follows
`mortar.update.channel`, by default the release channel `mortar`. To try builds on a
Brick before they are released, set `mortar.update.channel = "testing"` on it.

To release: **Actions > Promote to release > Run workflow**, with the commit
(`rev` in `channels/testing/<config>.json`; it must match). It needs no build: the
closure is in the bucket. The manifest is signed again, as the channel is part of what's
signed. To require a reviewer, add one to the `release` environment
(Settings > Environments). `mortar promote --from testing --rev COMMIT` does the same
from a computer.

## What is where

| Thing | Where | Secret? |
|---|---|---|
| Bucket name, endpoint, public signing key | `mortar.update` in your configuration | no |
| Signing key (secret half) | `~/.config/mortar/signing-key` on your computer; GitHub secret `NIX_SIGNING_KEY` | yes |
| R2 write token | GitHub secrets `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`; `~/.config/mortar/r2-write` on your computer | yes |
| R2 read-only token | the Brick: `/var/lib/mortar-update/credentials` (root, 0600, ext4 root fs) | yes |

No secret is ever in the repository or the Nix store.

## Runners and cost

On a public repository GitHub's standard hosted runners are free and unlimited, arm64
included: `ubuntu-24.04-arm` has 4 vCPUs, 16 GB RAM and 14 GB of SSD (the workflow frees
space first). A job can run for 6 hours. (A fork set to private pays per minute against
its plan and gets 2 vCPUs and 8 GB.) The first build compiles the kernel and the Rust
apps; later builds take unchanged paths from the bucket and only rebuild what changed.

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
are free. See [Cleaning the bucket](#cleaning-the-bucket).

## One-time setup

Run the Mac commands in this repository. `mortar` below is
`nix run .#mortar --`.

### 1. Bucket and tokens (Cloudflare dashboard)

1. **R2 object storage > Create bucket**: name it, e.g. `mortar-cache`.
   Leave public access off (no r2.dev URL, no custom domain).
2. On the R2 **Overview** page note your **Account ID**. The endpoint is
   `https://<ACCOUNT_ID>.r2.cloudflarestorage.com`.
3. **Account Details > API Tokens > Manage > Create Account API token**,
   twice:
   - `mortar-ci`: **Object Read & Write**, applied to `mortar-cache` only.
   - `mortar-brick`: **Object Read only**, applied to `mortar-cache` only.

   Each shows an **Access Key ID** and a **Secret Access Key** once. Keep
   them in your password manager.

### 2. Signing key (Mac)

```sh
mkdir -p ~/.config/mortar && chmod 700 ~/.config/mortar
nix key generate-secret --key-name mortar-1 > ~/.config/mortar/signing-key
chmod 600 ~/.config/mortar/signing-key
nix key convert-secret-to-public < ~/.config/mortar/signing-key   # -> mortar-1:…=
```

### 3. Your configuration

Set these in `modules/core/default.nix` for `brick-bare` and `brick-full`, or in a
module of your own flake:

```nix
mortar.update = {
  enable = true;
  bucket = "mortar-cache";
  endpoint = "https://<ACCOUNT_ID>.r2.cloudflarestorage.com";
  publicKey = "mortar-1:…=";   # the output of convert-secret-to-public
  # configName: brick-bare and brick-full set theirs; a flake of your own sets
  # it to the system's name under nixosConfigurations
};
```

Commit it (pushing triggers the first build; that is fine once step 4 is done). In a
public repository this makes the bucket's name and endpoint public, which is no secret
but is an invitation; they aren't credentials.

The `settings` step fails at once if this or a secret is missing.

### 4. GitHub secrets

```sh
gh secret set NIX_SIGNING_KEY -R gopherbone/mortar < ~/.config/mortar/signing-key
gh secret set R2_ACCESS_KEY_ID -R gopherbone/mortar        # mortar-ci Access Key ID
gh secret set R2_SECRET_ACCESS_KEY -R gopherbone/mortar    # mortar-ci Secret Access Key
```

(`gh secret set NAME` without `<` asks for the value.) Check that Actions
is enabled for the repository.

Builds on push stay off until you turn them on, so pushes don't spend
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
mortar publish            # build, push, publish; or --dry-run to see the manifest
```

### 6. The Brick, once over USB

The Brick needs a system that has `mortar-update` and trusts your key, so
deploy once from the Mac, and keep PICO-8 in its own profile:

```sh
mortar deploy --reboot
mortar pico8              # only if you use PICO-8 (needs your zip in the Mac's store)
```

Then give it the read-only token, either way:

- **From the Mac**: `mortar update-creds` (asks for the `mortar-brick`
  Access Key ID and Secret; sent over SSH on stdin, then checks the bucket).
- **On the card**: open the `NIXOS_BOOT` partition and add to `mortar.toml`:

  ```toml
  [update]
  access_key_id = "<mortar-brick Access Key ID>"
  secret_access_key = "<mortar-brick Secret Access Key>"
  # auto_check = false     # Settings > Auto-check daily starts off
  ```

  At the next boot the Brick moves the secret to
  `/var/lib/mortar-update/credentials` (root only) and blanks it in the file.

### 7. Try it

Push a commit, watch **Actions > Build and publish**, run **Promote to release**
(or point the Brick at `testing`), then on the Brick **MENU > Settings > Updates > A**
(Check now), or `mortar update` from your computer. When the toast says **Update ready — restart to apply**, Settings >
Updates says **Restart to update**; press A twice there (or restart any
other way).

## On the Brick

- **Settings > Updates**: the status; **A** checks now, or, with an update
  staged, restarts into it (press twice).
- **Settings > Auto-check daily**: on by default (`mortar.update.autoCheck`).
  The daily check (`mortar-update-auto.timer`, also 20 minutes after boot)
  downloads and stages an update by itself; it never restarts the Brick.
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
- The closure must be signed by the same keys. For that copy they replace what
  `nix.settings.trusted-public-keys` lists, so a path with cache.nixos.org's
  signature alone isn't accepted from the bucket.
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
  second buzz. That boots the previous generation, once; then roll back as above
  to make it stick. This needs a card with a U-Boot from after the feature was
  added (it isn't updated with the system: firmware/mainline/README.md).
- **If that doesn't help**: the U-Boot menu lists the last 5 generations, but only
  on the UART console. Without one, put the card in a computer, open
  `NIXOS_BOOT/extlinux/extlinux.conf` and change the `DEFAULT` line to the label of
  the previous entry (the `LABEL` lines below it, newest first), then boot and run
  `mortar-update rollback` to make it stick.
- **The channel itself**: publish an older system again, e.g.
  `mortar publish --path /nix/store/…-nixos-system-mortar-… --force` (the
  `toplevel` of an older manifest; `channels/mortar/<config>/` in the bucket keeps a
  copy of every manifest), or revert the commit and push. Publishing gives it a new
  date, so Bricks take it.

## Cleaning the bucket

Each publish adds only the store paths that changed, but nothing is ever
deleted. When the bucket grows too large, empty it (R2 dashboard > bucket >
Settings > delete objects, or a lifecycle rule) and re-run the workflow: it
pushes the whole current closure again. Bricks are unaffected: they already
have the paths they run.

## Rotating secrets

- Read token leaked or Brick lost: create a new **Object Read only** token,
  `mortar update-creds` on your Bricks, delete the old token.
- Write token: create a new one, update the two GitHub secrets (and
  `~/.config/mortar/r2-write`), delete the old one.
- Signing key: generate a new one (step 2). A Brick only trusts the keys of the
  system it runs, so first publish a system that has both,
  `mortar.update.trustedKeys = [ "old:…" "new:…" ]`, still signed with the old
  key, and wait for your Bricks to take it (or `mortar deploy` them). Then update
  `NIX_SIGNING_KEY`, set `publicKey` to the new key and drop the old one from
  `trustedKeys`, and empty the bucket so everything is re-signed.

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

The end-to-end test (`nix build .#checks.aarch64-linux.mortar-update`) runs
the whole path in VMs, with Garage as the S3 server: read key cannot write,
credentials from the card, unsigned, tampered, wrong-key, wrong-channel,
wrong-configuration and older manifests refused, unsigned closures and closures
signed by a key only `nix.conf` trusts refused, low battery, publish, download and
stage, offline, rollback, testing and promotion, auto-check.
