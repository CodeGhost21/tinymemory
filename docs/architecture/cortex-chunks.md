# CortexDB: chunked documents

How the CortexDB engine writes a document too long for one event, and how it
reads one back. The event layout and labels are in
[cortex-wire.md](cortex-wire.md); the request flows in
[cortex-flows.md](cortex-flows.md).

CortexDB refuses an experience whose flattened text is over 1 MiB
(`422 INVALID_ENVELOPE`, 0.10.4 API §6.10), and a document's event text is
its whole envelope. So (`envelope/chunks.rs`):

- **When.** Measured as a piece would be written (the envelope with its
  `chunk` field): a document whose encoded envelope fits in
  `DOCUMENT_CHUNK_TARGET_BYTES` (256 KiB) is one event, byte-identical to an
  unchunked one (no `chunk` field). A longer one is split. The target is the
  only granularity knob: at `0` every page and every section becomes its own
  event, except that a page or section too big for one event is still cut
  into several (see Where).
- **Where.** First at page breaks (the form feed the PDF converter puts
  between pages), then before markdown heading lines; a stretch of only
  whitespace (or page breaks) never becomes a piece of its own but joins the
  unit next to it, except in a text that is nothing else, which is then one
  piece, so every byte is kept. These units are packed greedily, in order,
  up to the target. A unit over the target is cut at blank lines, then line
  ends, then characters. Sizes are JSON-escaped bytes plus the envelope
  around the piece (its metadata and the `chunk` field at full width, with
  a page range reserved only when the document marks pages).
- **Limit.** No event over `MAX_EVENT_TEXT_BYTES` (768 KiB of encoded
  envelope, a quarter under the server's limit) is ever sent: every event of
  a batch is encoded and checked before the first write, and an item over it
  (a learning or a conversation turn that long) is `Error::InvalidRequest`.
- **Identity and replay.** Every piece carries the item's id and label, so
  replay detection, `forget` by id or filter, and `get` see all of them; a
  store that failed part way writes only the missing pieces. Until then
  `get` and `list` do not return the document (never a truncated body);
  `fetch` and `recall` still hit the pieces that are there.
- **Reads.** `get` and `list` give the whole document (pieces in index
  order). `fetch` and `recall` give one hit or citation per document, as for
  every item: its best-ranked piece, with the
  item's id and its metadata plus, when known, a `page:<n>` (or
  `page:<first>-<last>`) tag and a `section:<title>` tag: a document without
  page breaks gets no page tag, a piece before the first heading no section
  tag. These tags are read-side metadata, not part of the item's identity.
  `pages` is an inclusive range `[first, last]`, counted from 1, so a piece
  on one page has `first == last`. Readable CortexDB labels for page and
  section are not written yet.
- **Why one piece per target rather than per page.** CortexDB 0.10.4
  already fragments every event over about 500 bytes for retrieval
  (`matched_fragments`) and serves an over-budget event as an excerpt, and a
  hosted write is billed per event, so splitting is used only to stay under
  the limit, along the document's structure.
