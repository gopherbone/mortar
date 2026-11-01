# mortar.toml: the setup file

Set up a Brick before its first boot from any computer, the way Raspberry Pi
images work: flash the card, open its `NIXOS_BOOT` partition, edit
`mortar.toml`. New images carry a blank, commented copy. With
`mortar.setup.enable = true` (modules/mortar-setup.nix) the Brick applies it
at boot, before Wi-Fi, SSH and mDNS start.

```toml
hostname = "my-brick"            # -> my-brick.local

[wifi]
ssid = "Gloria_5G"
password = "correct horse battery"   # stored on the Brick, then blanked here
country = "US"                   # Wi-Fi regulatory domain (two letters)
hidden = false                   # the network does not broadcast its name

[ssh]
authorized_keys = [
  "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA… you@laptop",
]
password_login = true            # see below

[usb]
network = true                   # the Brick is a USB network adapter
```

Every key is optional; blank strings are ignored. A file with nothing set
(the shipped template) changes nothing and the Brick shows its Welcome
screen.

| Key | Effect |
|---|---|
| `hostname` | Host name and mDNS name (`<hostname>.local`). Letters, digits, dashes; a trailing `.local` is dropped. Default `mortar`. |
| `wifi.ssid`, `wifi.password` | Written to `/var/lib/iwd/<ssid>.psk` (or `.open` without a password) for iwd, which joins it automatically. Passwords are 8 to 63 characters. 2.4 GHz only (XR829). |
| `wifi.country` | `iw reg set` on every boot. |
| `wifi.hidden` | `Hidden=true` in the iwd network file. |
| `ssh.authorized_keys` | Public keys for root, added to `ssh-keys/root`. A single string is accepted too. Lines with options (`command=…`) are refused. |
| `ssh-keys/*.pub` | Any `.pub` file in the `ssh-keys/` folder next to `mortar.toml` is added the same way. `ssh-keys/root` itself is the file sshd reads. |
| `ssh.password_login` | `true` (default): while the pairing code is on the Brick's screen it is also root's SSH password over the USB cable (only), so plain `ssh root@mortar.local` or `ssh-copy-id` work. It turns itself off when the first key arrives. `false`: only keys and `mortar pair`. |
| `usb.network` | `false` turns the USB network off (the USB serial console stays). |

## What happens at boot

`mortar-setup-apply.service` runs before networkd, iwd, sshd and Avahi:

1. Reads `mortar.toml` and `ssh-keys/*.pub`.
2. If they changed since the last boot (a hash in
   `/var/lib/mortar-setup/state.json`), applies them: Wi-Fi network file,
   keys, switches, host name. Settings changed later on the Brick
   (Settings > Remote access) are therefore not undone on every boot, only
   when you edit the file again.
3. Replaces the Wi-Fi password in the file with `""` and a comment, keeping
   the rest of the file as you wrote it.
4. Every boot: sets the host name and Wi-Fi country, switches the USB
   network on or off.
5. Leaves a summary in `/run/mortar-setup/setup-file.json`; the first-boot
   screen shows it ("Found your setup file").

Problems (a bad key line, an invalid host name, a TOML syntax error) are
logged (`journalctl -u mortar-setup-apply`) and shown on that screen; the
rest of the file is still applied.

If keys arrive through the file and `password_login` is not set, password
login is turned off, as after pairing.

## The other ways in

- `mortar pair` over the USB cable (docs/pairing.md).
- Settings on the Brick: Wi-Fi network list with the on-screen keyboard or
  the router's WPS button; Remote access for keys and pairing codes.
