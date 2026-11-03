// SPDX-License-Identifier: GPL-2.0-or-later
// node --test test/worker.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import worker, { parseRange } from "../worker.js";
import { memoryBucket } from "./fake-bucket.mjs";

const HASH = "0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd";
const bytes = (s) => new TextEncoder().encode(s);
const objects = {
  "nix-cache-info": { bytes: bytes("StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 41\n") },
  [`${HASH}.narinfo`]: { bytes: bytes("StorePath: x\n"), cacheControl: "no-cache" },
  "nar/abc123.nar.zst": { bytes: bytes("0123456789"), contentType: "application/x-nix-nar" },
  "channels/testing/brick-bare.json": { bytes: bytes("{}"), cacheControl: "no-cache" },
  "channels/testing/brick-bare/1-abc.json": { bytes: bytes("{}") },
  "images/testing/brick-bare-latest.img.zst": { bytes: bytes("image") },
  "images/testing/brick-bare-latest.img.zst.sha256": { bytes: bytes("ff  x\n") },
  "images/testing/v/brick-bare-abc.img.zst": { bytes: bytes("image") },
  "secret/token": { bytes: bytes("must not be served") },
  "notes.txt": { bytes: bytes("must not be served") },
};

function ask(path, init = {}) {
  const bucket = memoryBucket(objects);
  return worker
    .fetch(new Request(`https://cache.example${path}`, init), { BUCKET: bucket })
    .then(async (r) => ({ r, text: r.body ? await r.text() : "", ops: bucket.ops }));
}

test("GET serves an object with its metadata", async () => {
  const { r, text } = await ask("/nar/abc123.nar.zst");
  assert.equal(r.status, 200);
  assert.equal(text, "0123456789");
  assert.equal(r.headers.get("content-length"), "10");
  assert.equal(r.headers.get("content-type"), "application/x-nix-nar");
  assert.equal(r.headers.get("accept-ranges"), "bytes");
  assert.match(r.headers.get("etag"), /^"[0-9a-f]{32}"$/);
});

test("HEAD has the headers, no body, and never reads the body", async () => {
  const { r, text, ops } = await ask("/nar/abc123.nar.zst", { method: "HEAD" });
  assert.equal(r.status, 200);
  assert.equal(text, "");
  assert.equal(r.headers.get("content-length"), "10");
  assert.ok(r.headers.get("etag"));
  assert.deepEqual(ops.map((o) => o[0]), ["head"]);
});

test("cache headers: immutable for store objects, short for channels", async () => {
  const cache = async (p) => (await ask(p)).r.headers.get("cache-control");
  assert.match(await cache(`/${HASH}.narinfo`), /immutable/); // and over the stored no-cache
  assert.match(await cache("/nar/abc123.nar.zst"), /immutable/);
  assert.match(await cache("/images/testing/v/brick-bare-abc.img.zst"), /immutable/);
  for (const p of ["/channels/testing/brick-bare.json", "/images/testing/brick-bare-latest.img.zst", "/images/testing/brick-bare-latest.img.zst.sha256"]) {
    const c = await cache(p);
    assert.match(c, /max-age=60\b/, p);
    assert.doesNotMatch(c, /immutable/, p);
  }
});

test("no-transform on everything Nix and resuming downloads count bytes of", async () => {
  for (const p of [`/${HASH}.narinfo`, "/nar/abc123.nar.zst", "/nix-cache-info", "/channels/testing/brick-bare.json"]) {
    assert.match((await ask(p)).r.headers.get("cache-control"), /no-transform/, p);
  }
});

test("content types fall back by kind", async () => {
  const type = async (p) => (await ask(p)).r.headers.get("content-type");
  assert.equal(await type("/nix-cache-info"), "text/x-nix-cache-info");
  assert.equal(await type(`/${HASH}.narinfo`), "text/x-nix-narinfo");
  assert.equal(await type("/channels/testing/brick-bare.json"), "application/json");
  assert.equal(await type("/images/testing/brick-bare-latest.img.zst"), "application/zstd");
  assert.match(await type("/images/testing/brick-bare-latest.img.zst.sha256"), /^text\/plain/);
});

test("Range", async () => {
  const range = async (v, extra = {}) => ask("/nar/abc123.nar.zst", { headers: { range: v, ...extra } });

  let { r, text } = await range("bytes=2-4");
  assert.equal(r.status, 206);
  assert.equal(text, "234");
  assert.equal(r.headers.get("content-range"), "bytes 2-4/10");
  assert.equal(r.headers.get("content-length"), "3");

  ({ r, text } = await range("bytes=7-"));
  assert.equal(text, "789");
  assert.equal(r.headers.get("content-range"), "bytes 7-9/10");

  ({ r, text } = await range("bytes=-2"));
  assert.equal(text, "89");

  // past the end is clamped
  ({ r, text } = await range("bytes=8-100"));
  assert.equal(r.status, 206);
  assert.equal(text, "89");

  ({ r } = await range("bytes=10-"));
  assert.equal(r.status, 416);
  assert.equal(r.headers.get("content-range"), "bytes */10");

  // several ranges or nonsense: the whole object is a valid answer
  for (const v of ["bytes=0-1,3-4", "items=0-1", "bytes=5-2", "bytes=-"]) {
    ({ r, text } = await range(v));
    assert.equal(r.status, 200, v);
    assert.equal(text, "0123456789", v);
  }

  // HEAD with a range says what GET would
  ({ r, text } = await ask("/nar/abc123.nar.zst", { method: "HEAD", headers: { range: "bytes=2-4" } }));
  assert.equal(r.status, 206);
  assert.equal(r.headers.get("content-length"), "3");
  assert.equal(text, "");
});

test("If-Range and If-None-Match", async () => {
  const etag = (await ask("/nar/abc123.nar.zst")).r.headers.get("etag");
  let { r, text } = await ask("/nar/abc123.nar.zst", { headers: { range: "bytes=2-4", "if-range": etag } });
  assert.equal(r.status, 206);
  ({ r, text } = await ask("/nar/abc123.nar.zst", { headers: { range: "bytes=2-4", "if-range": '"old"' } }));
  assert.equal(r.status, 200);
  assert.equal(text, "0123456789");
  ({ r, text } = await ask("/nar/abc123.nar.zst", { headers: { "if-none-match": etag } }));
  assert.equal(r.status, 304);
  assert.equal(text, "");
  ({ r } = await ask("/nar/abc123.nar.zst", { headers: { "if-none-match": '"other"' } }));
  assert.equal(r.status, 200);
});

test("parseRange", () => {
  assert.deepEqual(parseRange("bytes=0-0", 1), { offset: 0, length: 1 });
  assert.deepEqual(parseRange("bytes=-100", 10), { offset: 0, length: 10 });
  assert.equal(parseRange("bytes=-0", 10), "unsatisfiable");
  assert.equal(parseRange("bytes=0-", 0), "unsatisfiable");
  assert.equal(parseRange(null, 10), null);
});

test("404 cleanly, with a short cache", async () => {
  for (const p of [`/${"z".repeat(32)}.narinfo`, "/nar/nope.nar.zst", "/channels/testing/nope.json"]) {
    const { r, text } = await ask(p);
    assert.equal(r.status, 404, p);
    assert.equal(text, "Not found\n");
    assert.match(r.headers.get("cache-control"), /max-age=60/);
  }
  assert.equal((await ask("/nar/nope.nar.zst", { method: "HEAD" })).r.status, 404);
  assert.equal((await ask("/nar/nope.nar.zst", { headers: { range: "bytes=0-1" } })).r.status, 404);
});

test("nothing outside the cache's own kinds is served, and the bucket isn't asked", async () => {
  for (const p of ["/secret/token", "/notes.txt", "/channels/", "/nar/", "/images/", "/channels", "/nar", "/%2e%2e/secret/token", "/?list-type=2", "/nar/abc123.nar.zst/", "/channels/../secret/token"]) {
    const { r, ops } = await ask(p);
    if (p === "/?list-type=2") {
      assert.equal(r.status, 200); // the index page, not a listing
      assert.equal(ops.length, 0);
      continue;
    }
    assert.equal(r.status, 404, p);
    assert.equal(ops.length, 0, p);
  }
});

test("read-only: other methods are refused before the bucket", async () => {
  for (const method of ["PUT", "POST", "DELETE", "PATCH", "OPTIONS"]) {
    const { r, ops } = await ask("/nar/abc123.nar.zst", { method, body: ["PUT", "POST", "PATCH"].includes(method) ? "x" : undefined });
    assert.equal(r.status, 405, method);
    assert.equal(r.headers.get("allow"), "GET, HEAD");
    assert.equal(ops.length, 0);
  }
});

test("the index page says what this is", async () => {
  const { r, text } = await ask("/");
  assert.equal(r.status, 200);
  assert.match(r.headers.get("content-type"), /^text\/html/);
  assert.match(text, /github\.com\/gopherbone\/mortar/);
  assert.match(text, /brick-bare-latest\.img\.zst/);
  assert.match(text, /can't be sold/);
  assert.equal((await ask("/", { method: "HEAD" })).text, "");
});
