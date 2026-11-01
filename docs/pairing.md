# Pairing a computer: `mortar pair`

```console
$ mortar pair
Looking for Bricks on USB and the local network…
  ● mortar.local  USB · 10.55.0.1
Enter the code on the Brick's screen: TIDE-4782
✓ Host key matches the screen  SHA256:Qx3n…w8Ka
✓ Added ~/.ssh/id_ed25519.pub to the Brick
✓ Turned off password login
✓ Added "Host brick" to ~/.ssh/config
Connect any time with: ssh brick
```

The Brick shows the code on its first boot (USB cable screen) or from
Settings > Remote access > Pair a new computer. `mortar pair`:

1. finds the Brick at `10.55.0.1` (the USB link), then `mortar.local`
   (`--host` to choose);
2. asks for the code and runs the exchange below, which also proves that
   the SSH host key it receives belongs to the Brick showing that code, so
   no fingerprint needs comparing by eye;
3. uploads `~/.ssh/id_ed25519.pub` (or `id_ecdsa`, `id_rsa`; `--key FILE`;
   offers `ssh-keygen` if there is none) into `<card>/ssh-keys/root`;
4. pins the host key in `~/.ssh/known_hosts` under the alias and puts a
   managed block at the top of `~/.ssh/config` (`--name` picks the alias,
   `--no-ssh-config` skips this):

   ```
   # >>> mortar pair: brick (managed; this block is rewritten by `mortar pair`)
   Host brick
     HostName mortar.local
     User root
     HostKeyAlias brick
     IdentityFile ~/.ssh/id_ed25519
     IdentitiesOnly yes
   # <<< mortar pair: brick
   ```

Without the helper: while password login is on (the default until the first
key), `ssh root@mortar.local` or `ssh-copy-id root@mortar.local` over the
cable accept the code as the password.

## Protocol (mortar-pair/1)

TCP port 7330, newline-delimited JSON (pkgs/mortar-pair/src/lib.rs):

```
computer -> brick  hello                      is pairing open? (no secret)
brick -> computer  info {hostname, pairing, tries_left}
computer -> brick  start {spake: A}
brick -> computer  challenge {spake: B, hostname, host_keys, confirm}
computer -> brick  add_key {key, mac}
brick -> computer  done {key_fingerprint, password_login, addresses, mac}
```

- The code is the password of a SPAKE2 exchange (Ed25519 group, RustCrypto
  `spake2`). Both sides end with the same 256-bit secret only if they used
  the same code.
- HKDF-SHA256 over that secret, salted with a hash of the transcript (both
  SPAKE2 messages, the host name, every host key), gives three MAC keys:
  `confirm` (the Brick proves it knows the code, which authenticates the
  host keys it sent), `add_key.mac` (the computer proves it, which
  authorizes the key) and `done.mac` (the Brick confirms what it stored).

## Why a short code is enough

The code is a word from a list of 256 and four digits: about 21 bits.

- **No offline guessing.** An eavesdropper on the link learns nothing it
  can test codes against (that is what a PAKE is for). An active attacker
  gets one guess per connection.
- **Ten tries per code.** Each connection that gets as far as a challenge
  counts as a try (the Brick cannot tell a wrong guess until the MAC
  arrives, so it counts first). After 10 the code is dead and pairing
  stays locked until someone presses Y on the Brick. Ten guesses against
  2.56 million codes: 4 in a million. Connections are also limited to one
  per second and four at a time.
- **Short-lived.** A code expires after 15 minutes or as soon as one key
  has been added. With keys on the Brick, a code exists only after a
  button press on it (Pair a new computer), and leaving that screen ends
  it. Without keys (first boot) a code is always on offer, since the
  screen is the only way in.
- **USB only.** The service binds to `usb0` (`SO_BINDTODEVICE`) and refuses
  peers outside `10.55.0.0/24`. The cable means someone has the Brick in
  hand. (`mortar.setup.pairing.interfaces` could add `wlan0`; the
  arguments above hold on a shared network too, but it is off.)
- **Only keys.** The uploaded line must be a plain public key of a known
  type (no `command=`/`from=` options); it is appended to
  `<card>/ssh-keys/root` atomically.
- **Code as SSH password** (the `ssh root@mortar.local` path): sshd gets a
  `Match LocalAddress 10.55.0.1` block allowing password logins only while
  password login is on and a code is showing; a PAM rule
  (`pam_exec … mortar-setup check-password`) accepts exactly the current
  code, for root, from the USB subnet. Wrong passwords use up the same 10
  tries. Once a key exists, password login turns off; turning it back on
  is a switch on the Brick (Settings > Remote access).
- **Local access.** The screens talk to the daemon over
  `/run/mortar-setup/control.sock` (mode 0660, group `mortar-setup`). That
  socket can read the code, so the group is root-equivalent; only the
  session users of the Brick are in it.

## No hotspot: why the plan's phone page changed

The plan had the Brick open a temporary Wi-Fi hotspot for a phone. The
XR829 driver supports station mode only, so the Brick cannot be an access
point. Options considered:

| Option | Verdict |
|---|---|
| Phone joins a Brick hotspot | Impossible: no AP mode. |
| Bluetooth provisioning (BLE GATT from a web page) | Needs Web Bluetooth: Chrome on Android only, not iOS; a lot of moving parts for one form. |
| QR code that opens a setup page on the Brick | The phone has to reach the Brick first: only over a USB-C cable (phone as host), and then `mortar pair` from a computer is simpler. |
| WPS push button | iwd supports it in station mode (`SimpleConfiguration.PushButton`): press X on the Brick, then the router's WPS button. No typing. **Built.** |
| Network list + on-screen keyboard | The fallback that always works. **Built.** |

So the Wi-Fi path is: Welcome > Wi-Fi > pick a network (password on the
keyboard) or X for WPS. Without a key yet, the Brick then says how to add
one (cable + `mortar pair`, or `ssh-keys/` on the card), since pairing over
Wi-Fi is off by default.
