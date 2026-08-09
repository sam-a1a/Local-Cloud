# web

A page for driving one device, and the process that lets a page do that at all.

## Why there is a process here

A browser cannot join the mesh. The peer protocol is HTTPS with mutual TLS
against certificates pinned by pairing, on a port found over mDNS: a browser
will not present a client certificate from a store it does not have, will not
accept a self-signed server certificate, and cannot see mDNS. That is what the
protocol is for, and it is not something to work around.

So `web/src` is the mesh member. It runs a whole engine — it pairs, it is
discovered, it holds copies — and puts a plain HTTP API in front of it on
**127.0.0.1 only**, for a page it serves itself. The page is a view of this
device, not a peer.

Loopback is not a default to be relaxed. This API unpairs devices, deletes
copies off other machines, and hands back the contents of any file in the
catalog, with no authentication of any kind. It is safe because the operating
system will not route it off this machine.

## Running it

```
cd web/ui && npm install && npm run build
cargo run --release -p web
open http://127.0.0.1:7777
```

`--release` matters: chunking and hashing a file is the work, and unoptimised
SHA-256 turns a transfer that should take seconds into one that takes minutes.

State lives in `~/.localcloud`; files live in `~/LocalCloud`, which the engine
watches — so anything dropped in that folder is indexed and offered to the mesh
without the browser being involved. Pass a directory to put both somewhere else:
`cargo run -p web -- /tmp/second-device`.

For hot reload while changing the page, leave the server running and use
`npm run dev` in `web/ui`; Vite proxies `/api` to it.

## Shape

| | |
|---|---|
| `src/main.rs` | Starts the engine, binds loopback, wires the routes. |
| `src/snapshot.rs` | The catalog joined to its holders, once per change rather than once per render. |
| `src/events.rs` | Engine events, and the SSE stream that carries them. |
| `src/api.rs` | Everything the page can ask this device to do. |
| `src/assets.rs` | The built page, read once at startup and served from memory. |
| `ui/` | Solid, TypeScript, Tailwind. |

## What it costs

One `EventSource` for the life of the page, and no polling. The server holds
one snapshot, compares it to the last one it sent, and sends nothing when
nothing changed — so a quiet mesh is a silent socket. Block progress skips the
snapshot entirely and goes straight to the bar, because a large file produces a
thousand of those and none of them change the catalog.

The page applies each snapshot with Solid's `reconcile`, so a whole object
arriving updates only the signals that actually differ. Uploads and downloads
are streamed at both ends; neither side holds a file in memory.

At the time of writing: 16 kB of JavaScript and 5 kB of CSS, gzipped.
