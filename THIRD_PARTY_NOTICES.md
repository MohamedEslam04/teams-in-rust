# Third-party notices

IronTeams is under GPL-3.0-or-later (see [LICENSE](LICENSE)). Parts of it are adapted from the
projects below, under their own licenses. The notice each license asks for is kept here, and each
adapted piece of code says so in a comment next to it.

## rusty-teams

Source: <https://github.com/danieldaeschle/rusty-teams>

Adapted code (search the source for "rusty-teams" for every place, including the endpoint facts
and wire shapes that were read from it):

- `crates/calling/src/captions.rs`: `Depacketizer`, from `crates/calling/src/data_channel.rs`.
- `crates/calling/src/signaling/roster.rs`: `Roster::apply` and `apply_snapshot`.
- `crates/calling/src/signaling/bodies.rs`: `keep_alive_period`.
- `crates/calling/src/signaling/endpoints.rs`: `routed_url` and `ensure_teams_url`.
- `crates/calling/src/ngc_trouter.rs`: `inflated_json`.
- `crates/teams-proto/src/translate.rs`: `language_code` and its test, from `chatsvc/src/translate.rs`.
- `crates/teams-proto/src/urlpreview.rs`: link preview parsing and the showable rule, from `core/src/links.rs`.
- `crates/teams-proto/src/gifs.rs`: the GIF search request, from `chatsvc/src/gifs.rs`.
- `crates/teams-proto/src/global_search.rs`: the search request, from `substrate_search/request.rs`.

```
MIT License

Copyright (c) 2026 Daniel Däschle

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
