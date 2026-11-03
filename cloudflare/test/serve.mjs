// SPDX-License-Identifier: GPL-2.0-or-later
// The Worker over plain HTTP, for a directory: tests/mortar-update.nix points a real
// Nix and a real curl at the same code that answers at the public URL.
//   node serve.mjs DIR PORT
import { createServer } from "node:http";
import { Readable } from "node:stream";
import worker from "../worker.js";
import { dirBucket } from "./fake-bucket.mjs";

const [dir, port] = process.argv.slice(2);
const env = { BUCKET: dirBucket(dir) };

createServer(async (req, res) => {
  const response = await worker.fetch(
    new Request(`http://${req.headers.host}${req.url}`, { method: req.method, headers: req.headers }),
    env,
  );
  res.writeHead(response.status, Object.fromEntries(response.headers));
  if (response.body) Readable.fromWeb(response.body).pipe(res);
  else res.end();
}).listen(Number(port));
