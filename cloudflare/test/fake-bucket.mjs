// SPDX-License-Identifier: GPL-2.0-or-later
// A stand-in for an R2 binding: head() and get() as documented for R2, over whatever
// `load(key)` finds ({bytes, contentType?, cacheControl?} or null).
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { join } from "node:path";

export function fakeBucket(load) {
  const ops = [];
  const object = (found) => {
    const etag = createHash("md5").update(found.bytes).digest("hex");
    return {
      size: found.bytes.length,
      etag,
      httpEtag: `"${etag}"`,
      writeHttpMetadata(h) {
        if (found.contentType) h.set("content-type", found.contentType);
        if (found.cacheControl) h.set("cache-control", found.cacheControl);
      },
    };
  };
  return {
    ops,
    async head(key) {
      ops.push(["head", key]);
      const found = await load(key);
      return found ? object(found) : null;
    },
    async get(key, options) {
      ops.push(["get", key, options]);
      const found = await load(key);
      if (!found) return null;
      const r = options?.range;
      const bytes = r ? found.bytes.subarray(r.offset, r.offset + r.length) : found.bytes;
      if (r && r.offset >= found.bytes.length) throw new Error("R2 refuses this range: the Worker should have");
      return { ...object(found), body: new Response(bytes).body };
    },
  };
}

// A directory as a bucket: what the VM test serves.
export function dirBucket(root) {
  return fakeBucket(async (key) => {
    try {
      return { bytes: await readFile(join(root, ...key.split("/"))) };
    } catch {
      return null;
    }
  });
}

export function memoryBucket(objects) {
  return fakeBucket(async (key) => objects[key] ?? null);
}
