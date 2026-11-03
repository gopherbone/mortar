# SPDX-License-Identifier: GPL-2.0-or-later
# WPE Browser for TrimUI handhelds, https://github.com/matweew/wpe-tsp
# MIT, Copyright (c) 2026 Stanislav Matveyev (its LICENSE ships with the package).
{ fetchFromGitHub }:

fetchFromGitHub {
  owner = "matweew";
  repo = "wpe-tsp";
  rev = "ce09320b0376163217cdf6ee8660618e10f2d7cc";
  hash = "sha256-WZVsbsQKZCzs6EZRIMMEGytTUxLALwvmN5mOMRqZBqI=";
}
