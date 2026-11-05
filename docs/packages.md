# Packages

Installing more from the Brick itself, with the pad and no computer. A new core app lists
what's available; press A, and the package downloads in the background and appears under
Apps, with no system rebuild and no restart. The app and service are in
[pkgs/mortar-packages](../pkgs/mortar-packages/DESIGN.md).

**Not exercised yet.** All of this has run in VMs and on the shell's headless backend,
with made-up packages. CI publishes a catalog with a release candidate, but no Brick has
installed anything through it.

## What a repository is

A signed index plus a binary cache, like an apt repository. Each has a name, a URL, public
keys, a kind and a channel, set in `mortar.packages.repositories` and nowhere else: nothing
on the socket can add one. Mortar's own comes from `mortar.update` (`baseUrl`, `trustedKeys`,
`channel`, `upstreams`) and is the only one by default. A private bucket (S3 mode) has no
public index to read, so such a system lists nothing until it's given a repository.

## The catalog

`channels/<channel>/catalog.json`, one per channel, read through the Worker. A
`pkgs/mortar-index` format, shared with mortar-update and the `mortar` CLI.

```json
{
  "format": 1,
  "channel": "mortar",
  "published": 1793750400,
  "packages": [{
    "id": "my-port",
    "name": "My Port",
    "description": "What it is",
    "version": "1.2",
    "category": "games",
    "icon": "pad",
    "licence": "MIT",
    "source": "https://example.org/my-port",
    "kind": "nix",
    "requires": ["~/.local/share/my-port/data.bin"],
    "builds": [{ "device": "brick", "system": "aarch64-linux", "path": "/nix/store/…-my-port-app" }]
  }],
  "tools": [{ "system": "aarch64-linux", "path": "/nix/store/…-tools-index", "count": 93663 }],
  "signature": "mortar-1:…"
}
```

| `kind` | |
|---|---|
| `nix` (default) | Into `/nix/var/nix/profiles/apps/<id>`, where the shell finds it. Has to be a `lib.mkApp` package whose manifest says this ID. |
| `tool` | Into `/nix/var/nix/profiles/tools/<id>`, with a link per command in `/var/lib/mortar-packages/tools/bin`, at the end of Terminal's `PATH`. Not on the Apps page. |
| `bundle` | Parsed and listed as not installable yet. Nothing runs them. |

A device refuses a catalog that isn't signed by the repository's key, or is for another
channel, or is older than the last one it took from that repository (`published`, kept in
`/var/lib/mortar-packages/state.json`; it only ever goes up). What it refuses changes
nothing: the last good one stays. A format number from the future says "Update Mortar to
see what's new" and leaves everything alone. A package with no build for this device isn't
listed, and installing it by name is refused.

**Signatures say what they're for.** A signature covers a domain tag and then the body:
`mortar-manifest-1\n` for a system manifest, `mortar-catalog-1\n` for a catalog. The same
key signs both and store paths, so a catalog can't pass as a system update or the reverse,
and neither can pass as a path's (those start `1;/nix/store/`). Tests try each way, including
one document that parses as both.

### A large index

Every prebuilt terminal tool in nixpkgs: about 94,000 lines, 12 MiB unpacked. Too much to
hold as JSON on 969 MB, and a search has to look at all of it. So the catalog only names it:
`tools` points at a **store path** holding a sorted TSV:

```
#mortar-tools	1
ripgrep	ripgrep-14.1.1	0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd	rg	Fast line-oriented search
```

Attribute, store path name, store path hash, main program, description. It's fetched the
first time someone searches, with `nix-store --realise` against the repository's own key
and cache, compressed by the cache (3.5 MiB in the prototype), and held by a GC root.

Why a store path, and not a compressed file beside the catalog:

- One signed file per channel, so one rollback check, one signature, no second checksum.
- The Worker already serves narinfos and NARs. A new kind of file in `channels/` would need
  a change there.
- No zstd on the device, nothing to unpack and nowhere else to keep it. Old ones are collected
  like anything else.

That store path can't have references, or fetching it fetches every package it names. Nix
looks in an output for the hashes of the derivation's inputs, so the generator has to work
from evaluation alone, with the names as plain strings. The service refuses a list that would
bring anything else with it.

A search streams it through one 64 KiB buffer and keeps the best 50 hits; a test does that
on 14 MB counting every allocation, and peaks at about 80 KB. Its first line has a format
number, so a later format fails politely too.

A tool from this list installs against **the upstream's cache and key only**, not the
repository's: Mortar vouches for the list, and the package still has to carry cache.nixos.org's
signature. A tampered list can only point at packages that are already genuine.

## Publishing

`lib.mkCatalog` builds a catalog from `lib.mkApp` packages. It refers to every app, so its
closure holds them all; it has no date, channel or signature yet.

```nix
mortar.lib.mkCatalog pkgs {
  devices.brick = [
    { app = my-port-app; licence = "MIT"; source = "https://example.org/my-port"; }
    other-app  # version, licence and source from its package's meta
  ];
}
```

```
mortar publish --config brick-bare --channel testing --catalog ./result --catalog-only …
mortar promote --from testing --channel mortar --catalog-only …
```

`mortar publish --catalog` pushes what's listed, then signs and writes the catalog last, as
with a manifest; without `--catalog-only` it does so after the system. `mortar promote` signs
it again for the other channel. A list that hasn't changed isn't written.

CI's `packages` job does this for the official repository on a release candidate's tag, after
the systems: `packages.aarch64-linux.catalog` (the flake's list), and
`scripts/make-tools-index.sh`, which evaluates the nixpkgs the systems are built from with
`nix-eval-jobs`, asks cache.nixos.org for every path, and leaves out what it doesn't hold and
anything `scripts/check-public-closure.sh` refuses by name. A request that keeps failing, or a
list under 50,000, stops the job, so a network failure can't publish a short list.

What the catalog lists has to pass the same rules as a system: `tests/public-policy.nix`
evaluates it with `mortar.public` on, and CI runs the closure check on it.

## The system guard

Nix identifies a path by its name, not its contents, and takes whatever a trusted key signed.
So a repository could publish its own contents under a path a future Mortar system will use.
During the update Nix would see that path already there, keep it, and it would run as root at
boot. `mortar-update` looks at every path of a new system before staging it, against the
publisher's and the upstreams' keys and no others:

| A path of the new system | |
|---|---|
| Has a valid signature by one of those keys | Fine. `nix store verify` checks the cryptography: a repository can write any key's *name* on a signature. |
| Signed only by someone else | Its signatures are looked up in those caches (`nix store copy-sigs`), and only count if the NAR hash is the one here. The very same contents are no harm, and pass. Other contents **block the update** ("Update blocked"). |
| Has no signature at all | Passes, and the journal says how many. |

The last row is what keeps your own deploys working. A repository can't leave an unsigned path:
everything from a substituter arrives under `require-sigs` and keeps its signature. Unsigned
paths come from root on the device, and they're common. An SD image registers its whole store
with no signatures, `mortar deploy` and any other `nix copy --no-check-sigs` from a computer
leave them, and so does a local build. Refusing them would stop every card ever written from
updating, and root could do anything anyway.

If an update is blocked, it says which path. Remove what came from the other repository, collect
garbage and try again. `mortar-update check --allow-foreign-paths` takes it anyway, when you
know why: your own computer signing what it deploys with a key this Brick doesn't list is the one
case that gets here without a third party, if the build differs from the published one.

Deploys themselves don't go through this: `mortar deploy` runs nothing of mortar-update.

## Not here yet

Bundles' runtime and sandbox, PortMaster, third-party repository UI, `mortar install`,
and `mortar repo publish`. The hooks
are `kind`, `RepoKind`, `official`, and `tools` in the catalog.
