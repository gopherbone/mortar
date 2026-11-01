# Over-the-air updates

The Brick updates itself over Wi-Fi, without your computer:

```
git push (branch mortar)
  └─ GitHub Actions (.github/workflows/build.yml, arm64 runner)
       ├─ nix build …brick-mainline…toplevel
       ├─ nix copy --to s3://BUCKET (Cloudflare R2, private)   closure, signed with your Nix key
       └─ channels/mortar.json                                 manifest, written last
Brick (mortar-update, daily or Settings > Updates > Check now)
  ├─ reads channels/mortar.json with a read-only key
  ├─ nix copy --from s3://BUCKET <toplevel>                    Nix checks every signature
  ├─ system profile + switch-to-configuration boot             a new boot entry; the old ones stay
  └─ toast "Update ready — restart to apply"
```

`mortar publish` does the CI part from your Mac when CI is not available.

Nothing in the bucket or in CI is public: the system contains files you may
not redistribute (TrimUI's firmware and GPU blobs, cores with non-commercial
licences). **PICO-8 is never built by CI or put in the bucket**: it lives in
its own profile on the Brick (`/nix/var/nix/profiles/pico8`), installed from
your Mac with `mortar pico8`, and updates leave it alone.

## What is where

| Thing | Where | Secret? |
|---|---|---|
| Bucket name, endpoint, public signing key | `mortar.update` in `flake.nix` | no |
| Signing key (secret half) | `~/.config/mortar/signing-key` on the Mac; GitHub secret `NIX_SIGNING_KEY` | yes |
| R2 write token | GitHub secrets `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`; `~/.config/mortar/r2-write` on the Mac | yes |
| R2 read-only token | the Brick: `/var/lib/mortar-update/credentials` (root, 0600, ext4 root fs) | yes |
| PuzzleScript deploy key | GitHub secret `PUZZLESCRIPT_DEPLOY_KEY` | yes |

No secret is ever in the repository or the Nix store.

## Runners and cost

GitHub-hosted arm64 Linux runners (`ubuntu-24.04-arm`) are available to
private repositories on personal accounts. They are billed per minute
against the plan's included usage: Linux 2-core arm64 is $0.005/min (x64:
$0.006/min); GitHub Free includes $12 of compute a month for private repos,
Pro $18. Standard private-repo runners have 2 vCPUs, 8 GB RAM and 14 GB of
SSD (the workflow frees space first). The first build compiles the kernel
and the Rust apps (expect 1–2 hours, under a dollar); later builds take
unchanged paths from the bucket and only rebuild what changed.

Sources: [GitHub-hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners),
[Actions runner pricing](https://docs.github.com/en/billing/reference/actions-runner-pricing),
[Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions).

Fallbacks (Actions > Build and publish > Run workflow > runner):

- `x86-qemu`: an x86 runner building aarch64 under QEMU. Works, but many
  times slower; a full rebuild can hit the 6-hour job limit.
- `self-hosted`: your own runner with the label `mortar` (for example on the
  Mac, where Nix builds through the linux-builder).
- No CI at all: `nix run .#mortar -- publish` on the Mac.

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

### 3. flake.nix

Fill in the block next to `./modules/mortar-update.nix`:

```nix
mortar.update = {
  enable = true;
  bucket = "mortar-cache";
  endpoint = "https://<ACCOUNT_ID>.r2.cloudflarestorage.com";
  publicKey = "mortar-1:…=";   # the output of convert-secret-to-public
};
```

Commit it (pushing triggers the first build; that is fine once step 4 is done).

### 4. GitHub secrets

The PuzzleScript flake input is a private repository fetched over SSH, so
CI gets a read-only deploy key for it:

```sh
ssh-keygen -t ed25519 -N "" -C mortar-ci -f /tmp/puzzlescript-deploy
gh repo deploy-key add /tmp/puzzlescript-deploy.pub -R gopherbone/puzzlescript_pd -t mortar-ci
gh secret set PUZZLESCRIPT_DEPLOY_KEY -R gopherbone/trimui_nix < /tmp/puzzlescript-deploy
rm /tmp/puzzlescript-deploy /tmp/puzzlescript-deploy.pub

gh secret set NIX_SIGNING_KEY -R gopherbone/trimui_nix < ~/.config/mortar/signing-key
gh secret set R2_ACCESS_KEY_ID -R gopherbone/trimui_nix        # mortar-ci Access Key ID
gh secret set R2_SECRET_ACCESS_KEY -R gopherbone/trimui_nix    # mortar-ci Secret Access Key
```

(`gh secret set NAME` without `<` asks for the value.) Check that Actions
is enabled for the repository and that your plan's Actions budget allows
arm64 minutes.

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

Push a commit, watch **Actions > Build and publish**, then on the Brick
**MENU > Settings > Updates > A** (Check now), or `mortar update` from the
Mac. When the toast says **Update ready — restart to apply**, Settings >
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
- On SSH: `mortar-update status`, `mortar-update check`,
  `journalctl -u mortar-update -u mortar-update-auto`.

## Rolling back

Every update is a new system generation; the previous ones stay installed.

- **From the Mac**: `mortar rollback` (switches now), or
  `mortar generations` to look first.
- **On the Brick** (Terminal or SSH): `mortar-update rollback`, then restart:
  the previous generation becomes the boot entry. The same published
  update is not taken again; the next publish is.
- **If the new system does not boot**: the U-Boot menu lists the last 5
  generations, but only on the UART console. Without one, put the card in a
  computer, open `NIXOS_BOOT/extlinux/extlinux.conf` and change the
  `DEFAULT` line to the label of the previous entry (the `LABEL` lines below
  it, newest first), then boot and run `mortar-update rollback` to make it
  stick.
- **The channel itself**: publish an older system again, e.g.
  `mortar publish --path /nix/store/…-nixos-system-mortar-… --force` (the
  `toplevel` of an older manifest; `channels/mortar/` in the bucket keeps a
  copy of every manifest), or revert the commit and push.

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
- Signing key: generate a new one (step 2), put the new public key in
  `flake.nix`, update `NIX_SIGNING_KEY`, then `mortar deploy` once (a Brick
  only trusts the key of the system it runs), and empty the bucket so
  everything is re-signed.

## Formats

`channels/<channel>.json` (pkgs/mortar-update/src/manifest.rs):

```json
{
  "schema": 1,
  "channel": "mortar",
  "system": "aarch64-linux",
  "config": "brick-mainline",
  "toplevel": "/nix/store/…-nixos-system-mortar-26.05…",
  "published": 1793525400,
  "date": "2026-11-01T09:30:00Z",
  "rev": "fb4f6aa…",
  "label": "26.05…",
  "changelog": ["USB MIDI: ALSA sequencer in the kernel, loaded at boot; README"],
  "publisher": "github-actions"
}
```

The Brick's status, for the shell: `/run/mortar-update/status.json`
(`state`: unconfigured, idle, checking, downloading, installing, ready,
up-to-date, offline, low-battery, error).

The end-to-end test (`nix build .#checks.aarch64-linux.mortar-update`) runs
the whole path in VMs, with Garage as the S3 server: read key cannot write,
credentials from the card, unsigned closures refused, low battery, publish,
download and stage, offline, rollback, auto-check.
