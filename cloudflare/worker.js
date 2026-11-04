// SPDX-License-Identifier: GPL-2.0-or-later
// The public face of Mortar's update cache: read-only HTTP in front of an R2 bucket,
// which is a Nix binary cache plus channel manifests and SD images (docs/updates.md).
// Nothing here is trusted by a Brick: manifests and store paths are signed, and it
// checks them against keys of its own.
//
// GET and HEAD of known kinds of object, with Range. No listing, no writes.

const REPO = "https://github.com/gopherbone/mortar";
const IMAGE = "/images/testing/brick-bare-latest.img.zst";

const IMMUTABLE = "public, max-age=31536000, immutable, no-transform";
const SHORT = "public, max-age=60, no-transform";
const NOT_FOUND = "public, max-age=60";

// What the bucket may serve: anything else in it stays private, by default.
// [cache-control, fallback content type]
function kind(key) {
  if (key === "nix-cache-info") return ["public, max-age=3600, no-transform", "text/x-nix-cache-info"];
  if (/^[0-9a-z]{32}\.narinfo$/.test(key)) return [IMMUTABLE, "text/x-nix-narinfo"];
  if (/^nar\/[0-9a-z]+\.nar(\.[a-z0-9]+)?$/.test(key)) return [IMMUTABLE, "application/x-nix-nar"];
  if (/^channels\/[A-Za-z0-9._\/-]+\.json$/.test(key)) return [SHORT, "application/json"];
  if (/^images\/[A-Za-z0-9._\/-]+$/.test(key)) {
    const type = key.endsWith(".sha256") ? "text/plain; charset=utf-8" : "application/zstd";
    return [/-latest\./.test(key) ? SHORT : IMMUTABLE, type];
  }
  return null;
}

// "bytes=a-b", "bytes=a-" or "bytes=-n" against an object's size:
// {offset, length}, "unsatisfiable", or null to send the whole object (no Range,
// several ranges or one we can't read, all of which HTTP lets us ignore).
export function parseRange(header, size) {
  const m = header && /^bytes=(\d*)-(\d*)$/.exec(header.trim());
  if (!m || (m[1] === "" && m[2] === "")) return null;
  let start, end;
  if (m[1] === "") {
    const n = Number(m[2]);
    if (n === 0) return "unsatisfiable";
    start = Math.max(size - n, 0);
    end = size - 1;
  } else {
    start = Number(m[1]);
    end = m[2] === "" ? size - 1 : Math.min(Number(m[2]), size - 1);
    if (m[2] !== "" && Number(m[2]) < start) return null;
  }
  if (start >= size) return "unsatisfiable";
  return { offset: start, length: end - start + 1 };
}

function headers(object, cache, type) {
  const h = new Headers();
  object.writeHttpMetadata(h);
  if (!h.has("content-type")) h.set("content-type", type);
  h.set("cache-control", cache); // over whatever was stored with the object
  h.set("etag", object.httpEtag);
  h.set("accept-ranges", "bytes");
  return h;
}

const matches = (list, etag) => list.split(",").some((t) => t.trim() === "*" || t.trim() === etag);

async function serve(request, env, key) {
  const k = kind(key);
  if (!k) return notFound();
  const [cache, type] = k;
  const head = request.method === "HEAD";
  const conditional = request.headers.has("if-none-match");
  const ranged = request.headers.has("range");

  // Only a plain GET can go straight to the body.
  if (!head && !conditional && !ranged) {
    const object = await env.BUCKET.get(key);
    if (!object) return notFound();
    const h = headers(object, cache, type);
    h.set("content-length", String(object.size));
    return new Response(object.body, { headers: h });
  }

  const meta = await env.BUCKET.head(key);
  if (!meta) return notFound();
  const h = headers(meta, cache, type);
  if (conditional && matches(request.headers.get("if-none-match"), meta.httpEtag)) {
    return new Response(null, { status: 304, headers: h });
  }
  const ifRange = request.headers.get("if-range");
  const range = ifRange && ifRange !== meta.httpEtag ? null : parseRange(request.headers.get("range"), meta.size);
  if (range === "unsatisfiable") {
    h.set("content-range", `bytes */${meta.size}`);
    h.set("content-length", "0");
    return new Response(null, { status: 416, headers: h });
  }
  const length = range ? range.length : meta.size;
  const status = range ? 206 : 200;
  h.set("content-length", String(length));
  if (range) h.set("content-range", `bytes ${range.offset}-${range.offset + length - 1}/${meta.size}`);
  if (head) return new Response(null, { status, headers: h });
  const object = await env.BUCKET.get(key, range ? { range } : undefined);
  if (!object) return notFound();
  return new Response(object.body, { status, headers: h });
}

function notFound() {
  return new Response("Not found\n", {
    status: 404,
    headers: { "content-type": "text/plain; charset=utf-8", "cache-control": NOT_FOUND },
  });
}

const INDEX = `<!doctype html>
<html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Mortar update cache</title>
<body style="font: 16px/1.5 system-ui, sans-serif; max-width: 40em; margin: 2em auto; padding: 0 1em">
<h1>Mortar update cache</h1>
<p><a href="${REPO}">Mortar</a> is NixOS for the TrimUI Brick handheld. This is where a Brick
gets its over-the-air updates: a read-only Nix binary cache, with signed channel manifests.</p>
<p><a href="${IMAGE}">SD image (bare, testing channel)</a>, with its
<a href="${IMAGE}.sha256">sha256</a>. How to write it and what has been tested is in the
<a href="${REPO}#readme">README</a>.</p>
<p>Mortar can't be sold and takes no donations.</p>
</body></html>
`;

// What the Brick's pairing screen points at (MORTAR_SETUP_HELP, modules/core/setup.nix).
const PAIR = `<!doctype html>
<html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Pair a computer with Mortar</title>
<body style="font: 16px/1.5 system-ui, sans-serif; max-width: 40em; margin: 2em auto; padding: 0 1em">
<h1>Pair a computer with Mortar</h1>
<p>Connect the Brick's bottom USB-C port to your computer and open
<b>Settings &gt; Remote access &gt; Pair a new computer</b> (first boot starts there). You'll use
the code on its screen. Afterwards, <code>ssh root@10.55.0.1</code> works over the cable.</p>
<h2>macOS and Linux</h2>
<pre>ssh-copy-id root@10.55.0.1</pre>
<p>The password it asks for is the code. If it says there are no identities, make a key first
with <code>ssh-keygen -t ed25519</code>. When it asks about the host key, compare the
fingerprint with the one on the Brick's screen before saying yes. After a wrong code, the Brick
turns your computer away for a little while (<code>Connection reset</code>): wait a minute and try again.</p>
<h2>Windows (PowerShell)</h2>
<pre>ssh-keygen -t ed25519   # if you have no key yet
type $env:USERPROFILE\\.ssh\\id_ed25519.pub | ssh root@10.55.0.1 "mkdir -p ~/.ssh; cat &gt;&gt; ~/.ssh/authorized_keys"</pre>
<h2>With Nix</h2>
<pre>nix run github:gopherbone/mortar/mortar#mortar -- pair</pre>
<p>This checks the host key for you, and offers to set up Wi-Fi from your keyboard.</p>
<h2>No cable</h2>
<p>Put your public key in <code>ssh-keys/root</code> on the card's boot partition.</p>
<h2>Wi-Fi from your keyboard</h2>
<p>Once paired, instead of the on-screen keyboard:</p>
<pre>ssh -t root@10.55.0.1 mortar-setup wifi</pre>
<p><a href="/">Mortar update cache</a> · <a href="${REPO}">source</a></p>
</body></html>
`;

export default {
  async fetch(request, env) {
    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("Method not allowed\n", {
        status: 405,
        headers: { allow: "GET, HEAD", "content-type": "text/plain; charset=utf-8" },
      });
    }
    const url = new URL(request.url);
    if (url.pathname === "/") {
      return new Response(request.method === "HEAD" ? null : INDEX, {
        headers: { "content-type": "text/html; charset=utf-8", "cache-control": "public, max-age=3600" },
      });
    }
    // Raw: every key the bucket may serve is plain ASCII, with no escapes to read.
    if (url.pathname === "/pair") {
      return new Response(request.method === "HEAD" ? null : PAIR, {
        headers: { "content-type": "text/html; charset=utf-8", "cache-control": "public, max-age=3600" },
      });
    }
    return serve(request, env, url.pathname.slice(1));
  },
};
