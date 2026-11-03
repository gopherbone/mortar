# SPDX-License-Identifier: GPL-2.0-or-later
# What the published systems follow: brick-bare and brick-full take this, and nothing
# else does. A Brick with it replaces its whole system with what's published, so it
# is not a flake output, and `nixosModules.full` doesn't include it: a flake of your
# own with more in it must not end up here. tests/public-policy.nix checks that.
#
# Keep it to the update settings: nothing here is secret, and the bucket's account
# isn't named. It's read through the Worker, with no credentials (docs/updates.md).
let
  # To release: set this to "mortar", push, try what that builds from `testing`, then
  # run promote.yml. A Brick follows what its own system says, so a system that says
  # "testing" must not be promoted, and promote.yml refuses one that does.
  channel = "testing";
in
{
  mortar.update = {
    enable = true;
    baseUrl = "https://mortar.angelfire.workers.dev";
    publicKey = "mortar-1:g6azCIBbBdslv81kYgZzgv7sAFYBPAnINCG8PUGFyaY=";
    inherit channel;
    # `testing` takes every push that passes CI, with no one having run it on a Brick.
    # A stranger's Brick shouldn't stage that by itself, so while the channel is
    # `testing` the daily check starts off (Check now and the setting still work).
    # The release channel only moves by hand, so there it starts on: it stages, it
    # never restarts, and holding SELECT at power-on boots the previous generation.
    autoCheck = channel != "testing";
  };
}
