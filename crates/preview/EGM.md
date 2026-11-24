# preview

## Overview
IronTeams file preview: renders PDF pages to RGBA bitmaps in pure Rust (`hayro`, no system library) and converts Office and other documents to PDF through Microsoft Graph (via `onedrive` and `teams_proto`). Rendering is CPU-bound and should run off the UI thread; `Document` is `Send + Sync` and pages render independently.

## Interface
- `Error {Invalid, PasswordProtected, Encrypted, NoPage{page, count}, BadWidth{width}, Unsupported(String), Io, Graph(teams_proto::Error)}` and `Result<T>` [lib.rs].
- Re-exports [lib.rs]: `CONVERTIBLE, can_convert, office_to_pdf, pdf_url` from `office`; `Document, MAX_SIDE, Rgba` from `pdf`.
- `Document::open(Vec<u8>)`, `open_path(&Path)`, `page_count`, `page_size(page) -> (f32, f32)` (points), `render_page(page, width) -> Rgba` [pdf.rs].
- `Rgba {width, height, pixels}` with `to_png() -> Result<Vec<u8>>`; `MAX_SIDE = 8192` [pdf.rs].
- `CONVERTIBLE` extensions list, `can_convert(name)`, `pdf_url(drive_id, item_id)`, `office_to_pdf(client, drive_id, item_id, name) -> Result<Vec<u8>>` [office.rs].

## Invariants
- Render width must be in `1..=MAX_SIDE`, else `Error::BadWidth`; height keeps the aspect ratio and is clamped to 8192 (the width is recomputed for very tall pages) [pdf.rs:render_page].
- Pages are 0-based; out of range gives `Error::NoPage` [pdf.rs:Document::page].
- Pages are drawn on opaque white; output is 8-bit non-premultiplied RGBA; crop box and rotation are applied [pdf.rs:render_page].
- Open errors map: password-protected to `PasswordProtected`, other decryption failures to `Encrypted`, invalid to `Invalid`; PDFs with an empty user password open [pdf.rs:Document::open].
- Known `hayro` limits: no blending or isolation groups, no knockout groups, no colour-key masking [lib.rs docs].
- `can_convert` and `office_to_pdf` accept only extensions in `CONVERTIBLE` (the extension is lowercased text after the last dot); anything else returns `Error::Unsupported` before any request [office.rs:office_to_pdf, extension].
- HTTP 406 from Graph maps to `Error::Unsupported`; other failures to `Error::Graph` [office.rs:conversion_error].
- `pdf_url` uses `/me/drive/items/{id}` when `drive_id` is empty; otherwise the `onedrive::ItemRef` URL plus `/content?format=pdf` [office.rs:pdf_url].

## Side Effects
- Network: `GET <item url>/content?format=pdf` through `onedrive::api::graph_raw` with a `teams_proto::Client`; read-only for the file [office.rs:office_to_pdf].
- Disk: `Document::open_path` reads the file with `std::fs::read` [pdf.rs:open_path].
- No env vars, no global state, no logging.

## Verification
- Run: `cargo test -p preview`
