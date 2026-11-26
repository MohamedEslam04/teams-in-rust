# Teams chat protocol notes

Everything here is the **internal** API of the Teams web client. Microsoft does not document it
and can change it at any time. IronTeams implements it in `crates/teams-proto`. The baseline comes
from [purple-teams](https://github.com/EionRobb/purple-teams) (GPL-3.0). The rest comes from
observing a real work account.

Status legend:
- ✅ verified against a real account (date)
- 🧪 unit-tested with fixtures only
- ⚠️ unverified

Fixtures: `crates/teams-proto/src/*` tests hold sanitized JSON. Raw captures (`fixtures/raw/`,
`*.har`) are git-ignored. They contain message content and must never be committed.

## Sign-in ✅ 2026-11-18

| Step | Request | Notes |
|---|---|---|
| Device code | `POST login.microsoftonline.com/organizations/oauth2/devicecode`, form `client_id=1fec8e78-…&resource=https://api.spaces.skype.com` | v1 endpoint. `interval` and `expires_in` may be strings |
| Poll | `POST …/oauth2/token`, `grant_type=urn:ietf:params:oauth:grant-type:device_code` | `authorization_pending`, `slow_down`, `expired_token` |
| Refresh | `POST …/oauth2/v2.0/token`, `grant_type=refresh_token&scope=<resource>/.default openid profile offline_access` | Done once per resource (spaces, `chatsvcagg`, presence). Persist the rotated refresh token |
| Skype token | `POST teams.microsoft.com/api/authsvc/v1.0/authz` with `Authorization: Bearer <spaces token>` | `tokens.skypeToken`, `tokens.expiresIn`, `region`, `regionGtms.chatService`, `regionGtms.middleTier` |

The region matters. The test tenant is in `amer`, while purple-teams hardcodes `apac`. IronTeams takes
the chat-service and middle-tier hosts from `regionGtms`.

## Chat service (`<chatService>` = `https://<region>.ng.msg.teams.microsoft.com`)

Headers: `X-Skypetoken`, `BehaviorOverride: redirectAs404`, `X-Stratus-Caller: skypeteams`,
`Origin`/`Referer: https://teams.microsoft.com`.

| Action | Request | Status |
|---|---|---|
| Conversations | `GET /v1/users/ME/conversations?view=msnp24Equivalent&pageSize=200&startTime=1&targetType=…` | ✅ |
| History | `GET /v1/users/ME/conversations/{id}/messages?view=msnp24Equivalent\|supportsMessageProperties&pageSize=N&startTime=1[&endTime=ms]` | ✅ |
| Send | `POST …/{id}/messages` `{clientmessageid, content, messagetype:"RichText/Html", contenttype:"text", imdisplayname[, properties.subject]}` | ✅ (self notes). Sending the same `clientmessageid` twice posts two messages (checked live in `48:notes`), so a retry must first look for the echo |
| Edit | `PUT …/{id}/messages/{msgId}` `{content, messagetype, contenttype, imdisplayname}` | ✅ (self notes) |
| Delete | `DELETE …/{id}/messages/{msgId}?behavior=softDelete` | ✅ (self notes) |
| React | `PUT …/{id}/messages/{msgId}/properties?name=emotions` `{"emotions":{"key":"like","value":<ms>}}` | ✅ (self notes) |
| Un-react | `DELETE …/{id}/messages/{msgId}/properties?name=emotions` `{"emotions":{"key":"like"}}` | ✅ (self notes) |
| Read marker | `PUT …/{id}/properties?name=consumptionhorizon` `{"consumptionhorizon":"<id>;<ms>;<cmid>"}` | ✅ (horizon equals the newest message after the app opened the chat) |
| Typing | `POST …/{id}/messages` `{messagetype:"Control/Typing", contenttype:"Application/Message", content:""}` | ⚠️ |
| Self | `GET /v1/users/ME/properties` → `skypeName` (`orgid:<guid>`) | ✅ |

### Channels ✅

- The channel id is `19:…@thread.tacv2`. History returns root posts and replies mixed together.
  Each message's `conversationLink` ends with `;messageid=<rootId>`, and a root's
  `rootMessageId` equals its own `id`.
- Reading a thread: `GET …/conversations/{channel};messageid={root}/messages`.
- Posting a reply: `POST …/conversations/{channel};messageid={root}/messages`.
- Posting a new post: `POST …/conversations/{channel}/messages`, with `properties.subject` for the
  title.
- Posting verified 2026-11-18 in a test channel: a new post with `properties.subject` (stored as `subject`, `rootMessageId` equal to its own id) and a reply in its thread.

### Message content

| `messagetype` | Rendering |
|---|---|
| `RichText/Html`, `Text` | HTML subset: `<p> <b> <i> <a> <ul> <pre>`, mentions `itemtype=…/Mention`, emoji `<img itemtype=…/Emoji alt=…>`, reply `<blockquote itemtype=…/Reply>` |
| `RichText/Media_Card` | `<URIObject type="SWIFT.1">…<Swift b64="…">` holds the Adaptive Card JSON in base64. Some messages carry `properties.cards` instead ✅ |
| `properties.cards` → `application/vnd.microsoft.card.audio` / `.video` | Voice message or video. `content.media[0].url` is an AMS object and `duration` is ISO-8601 ✅ |
| `properties.files` | JSON string, `[{fileName, fileType, objectUrl, fileInfo.shareUrl}]` (SharePoint/OneDrive) ✅ |
| `properties.emotions` | Array, or a JSON string of an array, of `{key, users:[{mri,time,value}]}` ✅ |
| `properties.edittime` / `deletetime` | Edited / deleted markers ✅ |
| `ThreadActivity/*`, `Event/Call` | System lines |

Sending a sticker (composer Stickers tab, `backend/stickers.rs`): a `RichText/Html` message that is only
`<img src="https://statics.teams.cdn.office.net/evergreen-assets/stickerassets/<category lowercase>-250x250/<file>?v=5" width="250" height="250" alt="<name>" itemtype="http://schema.skype.com/Sticker">`;
nothing is uploaded. The image CDN needs no sign-in. Shape and catalogue from rusty-teams (MIT); 🧪 UNVERIFIED by IronTeams
(nothing sent live). A Praise card (the Praise app's Adaptive Card) is not documented in the references, so IronTeams has none.

### Delivery options (9.4)

`crates/teams-proto/src/delivery.rs` (`ironteams-cli deliver`). Sources: the Teams web client JS bundle
(message serializer, composer and renderer); `properties.importance` was also known from
better-teams and Maxim.

| Option | Wire shape | Live status |
|---|---|---|
| Standard | `properties.importance` is `""` (web always sends it; IronTeams sends nothing, which reads the same) | ✅ default sends |
| Important | `properties.importance: "high"`. Web shows a red "IMPORTANT!" label | ✅ sent to `48:notes` 2026-11-19 and read back through history as `high`. Whether Teams web draws the label still needs a person to look |
| Urgent | `properties.importance: "urgent"`. The receiving web client repeats the notification until the message is acknowledged (its `ackUrgentMessage` runs once the reader has reacted to or seen it; an urgent message from the current user is never acked) | ✅ sent to `48:notes` 2026-11-19 and read back as `urgent`. The repeated notification is only seen by another recipient, so it was not observed |
| Announcement | Channel posts only. `properties.title` is the headline and `properties.cards` (JSON *string*) holds one card `{appId:"announcement-card", cardClientId:"announcement-card-<id>", contentType:"application/vnd.microsoft.teams.messaging-announcementBanner", appName?, content}`. `content` is `{colorTheme:"<0..11>"}` for a coloured banner (the 12 palette entries, from periwinkle blue to ocean green) or `{imageData:{origImage:{src,id?}, croppedImage:{src,id?,imageWidth,imageHeight,imageAltText}, isTeamsIllustration?}}` for a picture (web crops to 1317x366). A picture is uploaded to AMS first and its object id goes in the top-level `amsreferences`. `messagetype` stays `RichText/Html` | ⚠️ unverified live (not posted to any channel). Shape taken from the web bundle only; covered by unit tests |

Receive side: `properties.importance` is `""`, `"high"` or `"urgent"` (the numeric forms 2 and 3 also
appear internally). The web client builds an announcement from `properties.title` plus the card
above (it reads `colorTheme` from `imageData.colorTheme` first, then from the content). A message is
only an announcement when it has the card; a title alone is not one. `model::Message` carries
`importance` and `announcement`. Live read-back: `ironteams-cli deliver show <conversation>`.

## Aggregator (CSA) ✅

`GET teams.microsoft.com/api/csa/api/v1/teams/users/me?isPrefetch=false&enableMembershipSummary=true`,
with `Authorization: Bearer <chatsvcagg token>`.

The response has `teams[].channels[]` and `chats[]` (`title`, `isOneOnOne`, `members[].mri`).

`hidden: true` is set on many active 1:1 chats, so it is not a visibility flag. Use
`isConversationDeleted` instead.

## Profiles and avatars ✅

- Profiles: `POST <middleTier>/beta/users/fetchShortProfile?…` with a JSON array of MRIs. The
  response is `value[]` with `displayName` and `email`.
- Avatars: `GET <middleTier>/beta/users/{selfGuid}/profilepicturev2/{mri}?size=HR196x196` (lists,
  chat, cards; 64 px looked blurry at 2x), and `size=HR648x648` for the photo preview. The service
  offers HR64x64, HR96x96, HR196x196, HR432x432 and HR648x648. Avatars stored before this change
  are 64 px and are read again when first shown.
- Team pictures (channel rows of the chat list, `mt::team_picture`):
  `GET <middleTier>/beta/users/{selfGuid}/profilepicturev2/teams/{groupId}?displayName=<team name>&size=HR196x196`
  (the `size` is an assumption carried over from the user form: if the read fails it is repeated without it),
  the group id being the CSA `teamSiteInformation.groupId`. From the reference API description
  (`fetchTeamPicture`, which also passes `etag=<pictureETag>`; IronTeams does not read the etag yet).
  Fetched once per run, kept in memory only; a refusal keeps the initials. **UNVERIFIED**: not
  sent to any account.
- Profile card directory details (2026-11-20, **UNVERIFIED** live, read only):
  Graph `GET /v1.0/users/{oid}?$select=jobTitle,department,companyName,officeLocation,businessPhones,mobilePhone`
  and `GET /v1.0/users/{oid}/manager?$select=displayName`, sent together with the Delve card and
  the presence lookup (`card::Client::directory_details`). Documented Microsoft Graph reads; whether
  the app's `.default` Graph token may read other users (`User.ReadBasic.All` covers neither phones
  nor the manager) is not known, so a refusal leaves those card rows hidden. The card shows no
  local time or time zone: no documented read gives another person's zone.

## AMS (images, voice) ✅

- **Download:** `GET https://{us,eu,as,…}-api.asm.skype.com/v1/objects/{id}/views/imgo` (or
  `/views/audio`) with `Cookie: skypetoken_asm=<skypetoken>`. IronTeams only sends this credential to
  AMS hosts.
- **Upload an image:**
  1. `POST as-prod.asyncgw.teams.microsoft.com/v1/objects/` `{type:"pish/image", permissions:{<conv>:["read"]}, filename, sharingMode:"Inline"}` → `id`
  2. `PUT …/v1/objects/{id}/content/imgpsh` with the bytes
  3. Send `<img itemtype="http://schema.skype.com/AMSImage" src=".../views/imgo">` ✅ (self notes)
- **Upload a voice message** (`teams_proto::ams::upload_voice`, `voice::voice_message_body`) ✅ type
  confirmed 2026-11-21 with `ironteams-cli ams-meta` on a voice message Teams mobile sent to the own
  notes chat: the object is `{"type":"sharing/audio", "contents":{"audio":{…},
  "transcript_info":{…}}}` (Teams fills `transcript_info` itself). The message is `RichText/Html`
  whose body is only an `InputExtension` span, `properties.cards` a JSON string with one
  `application/vnd.microsoft.card.audio` card `{duration:"PT3S", media:[{url:
  "https://<region>-api.asm.skype.com/v1/objects/<id>/views/audio"}]}` and `amsreferences`
  naming the object (mobile listed three objects; only one is in the card).
- **Views of one object** (`GET …/v1/objects/<id>/views/<view>`, `teams_proto::images`) ✅ 2026-11-19 live, on images in the own notes chat:

  | View | Answer |
  |---|---|
  | `imgpsh_fullsize` | the **original**: 2160x2880 JPEG, 993309 bytes (a 1732x1299 JPEG answered 132802 bytes). The Teams web viewer uses this one. Content type is right (`image/jpeg`, `image/png`) |
  | `imgo` | what inline `<img src>` points at: a downscaled copy, 95308 bytes for the same 2160x2880 picture |
  | `imgt1` | thumbnail, 23249 bytes for the same picture |
  | `imgpsh` | HTTP 400 (it is the upload content name, not a view) |

  `<img>` tags carry the object id twice (`itemid` and `id="x_<id>"`) and a display size in
  `width`/`height`. The size can be a fraction (`383.0313014827018`) and its aspect ratio equals the
  original's, so it is good for sizing a placeholder; only the original has the real pixel size.
  Pasted images say `alt="image"`, forwarded ones `alt="Media"` or `"shared image"`. IronTeams tries
  `imgpsh_fullsize`, then `imgo`.
- **Token for AMS:** no bearer audience. The request carries the skypetoken twice, as
  `Cookie: skypetoken_asm=<skypetoken>` and `X-Skypetoken: <skypetoken>` (what `Client::bytes` adds
  for AMS hosts). The credential only goes to hosts in the AMS allow-list (`ams::is_ams_url`). The region of the object id
  (`0-eus-d11-…` is `eus`, United States) matches the host in the tag (`us-api.asm.skype.com`);
  the mapping for ids without a `src` (`ams_url_for_object`) is only checked for `eus`.
- **OneDrive/SharePoint images** (an image attached as a file, `properties.files`): 🧪 not run live, the
  own notes chat has no image file. IronTeams asks Graph first,
  `GET graph.microsoft.com/v1.0/shares/u!<base64url of the file URL>/driveItem/content` with the
  Graph bearer (the documented shares API; the redirect to the pre-authenticated download URL
  drops the bearer), and falls back to the file URL with a bearer for the SharePoint host.
  Images from any other host are fetched without any credential.
- **Files** are not AMS: see [Files](#files--2026-11-18) below.

## Video clips and Loop paragraphs (feat/clip-loop) 2026-11-20

### Video clips (`teams_proto::video_clip`, "Record video clip") **UNVERIFIED**

Nothing here has been sent live. No capture of the Teams client sending a video clip is in
`docs/`, `plans/` or the reference clones; the shape below is what the mautrix-teams bridge sends
for a video and what its parser and the rusty-teams client read back.

| Step | Request | Status |
|---|---|---|
| Create the object | `POST as-prod.asyncgw.teams.microsoft.com/v1/objects/` `{type: "sharing/videototranscode", permissions: {<conv>: ["read"]}, filename, sharingMode: "Inline"}` → `id`. `videototranscode` makes AMS prepare the `video`, `thumbnail` and lower-resolution views (mautrix-teams comment). Constants `VIDEO_OBJECT_TYPE` / `VIDEO_CONTENT` | **UNVERIFIED** placeholder, as `AUDIO_OBJECT_TYPE` for voice: confirm with `ironteams-cli ams-meta` on a clip Teams sent before trusting it. mautrix sends no `sharingMode` for videos; IronTeams's shared upload sends `Inline` |
| Upload | `PUT …/v1/objects/{id}/content/original` with the MP4 (H.264 video, AAC audio, `+faststart`) | **UNVERIFIED** (slot from mautrix-teams) |
| Message | `RichText/Html`, content `<video src="https://as-api.asm.skype.com/v1/objects/{id}/views/video" itemscope="" itemtype="http://schema.skype.com/AMSVideo" width="640" height="360" data-duration="PT12.40S">Video clip</video>`, `amsreferences: [id]` (`video_clip_message_body`) | **UNVERIFIED**. The native Teams clip may instead be a `card.video` card or carry a `VideoMsg` preview image (an itemtype the mautrix parser lists); the `<video>` tag is what mautrix-teams sends and parses |
| Thumbnail | `GET …/views/thumbnail` of the same object (rusty-teams `ams_video.rs`) | **UNVERIFIED** in IronTeams; fetched like an inline image, only for received clips |

IronTeams records the clip itself: the call camera (`calling::video::camera`, Camera portal +
PipeWire, else V4L2) at 640x360/15 fps piped into the system `ffmpeg` (`-c:v h264`, whichever H.264
encoder it has), the microphone through `voice::Recorder`, then a second `ffmpeg` run adds the
audio as AAC. Clips stop themselves at 60 s, as in Teams. When Teams refuses the upload or the
message, the row shows "Failed to send" with Retry and Delete, and a notice says sending clips is
not confirmed to work yet.

Received clips (`split_video`): an AMS `<video … AMSVideo>` tag, or a
`application/vnd.microsoft.card.video` card (already turned into a "🎬 Video" line and a
`kelma-media:` link by `cards::attachments_html`), become a clip card with the thumbnail and a play
button; playing downloads `views/video` once into the media cache and opens the system player
(no inline video decoding).

### Loop paragraphs (composer Loop button) — sent as formatted messages

A Loop component in Teams is a `.loop` file in a SharePoint Embedded container, kept live through
Fluid, and the chat message only links to it (`docs/research/apps-onedrive-copilot-store.md`, Loop:
SharePoint Embedded, no known public write path; Graph's `FileStorageContainer.Selected` only
reaches containers of the caller's own container types). IronTeams therefore does **not** create a
Loop component. The paragraph is sent as an ordinary `RichText/Html` message through the outbox
(`crate::loop_compose::to_html`):

- title: `<p><b>…</b></p>`; text: one `<p>` per line;
- Table: `<table><tbody><tr><th>…</th></tr><tr><td>…</td></tr></tbody></table>` (empty rows dropped);
- Bulleted list: `<ul><li>…</li></ul>`; Checklist: `<p>☐ item</p>` / `<p>☑ item</p>`;
- Task list: a table `Task | Assigned to | Due date`, the task prefixed ☐ / ☑;
- Voting table: a table `Option | Votes`.

The editor says so ("Sent as a formatted message: live Loop components need Microsoft Loop"), and
so does the notice after sending. Received Loop components keep their `LoopCard`.

## Files ✅ 2026-11-18

Chat files live in the sender's OneDrive, in `Microsoft Teams Chat Files`. The Teams client id
gets a Graph token through the usual refresh (`scope=https://graph.microsoft.com/.default`), and it
carries `Files.ReadWrite.All`.

1. Upload:
   - Up to 4 MB: `PUT graph.microsoft.com/v1.0/me/drive/root:/Microsoft Teams Chat Files/<name>:/content?@microsoft.graph.conflictBehavior=rename`.
   - Larger files: `POST …:/createUploadSession`, then `PUT` chunks (multiples of 320 KiB) with
     `Content-Range` to the returned `uploadUrl`. That URL is pre-authenticated, so it gets no token.
2. `GET /me/drive/items/{id}?$select=id,name,webUrl,sharepointIds`.
3. Share with the other chat members: `POST /me/drive/items/{id}/invite`
   `{recipients:[{objectId}], requireSignIn:true, sendInvitation:false, roles:["write"]}`. The object
   id is the GUID in `8:orgid:<guid>`. Self notes skip this step.
4. Send a message with empty content and `properties.files` = a JSON string of
   `[{"@type":"http://schema.skype.com/File", id: listItemUniqueId, baseUrl: siteUrl/, objectUrl: webUrl, fileName, fileType, fileInfo:{fileUrl, siteUrl, shareUrl}, …}]`.

Verified in self notes: a 55-byte file with an Arabic name, and a 5.5 MB file uploaded in two
chunks. The sizes in OneDrive matched, and both came back as file cards. The invite step (step 3)
is unit-tested only, because a live test would share a file with a colleague. Channel files live
in the team's SharePoint site, see "Chat list, channel posts and cards" below.

## Auth broker ✅ 2026-11-18

`teams_proto::broker` mints one bearer per audience from the refresh token on first use, caches it with
a wall-clock expiry (refreshed 5 minutes early) and rotates the refresh token on every mint.
`Client::request_as(audience, …)` only attaches the bearer to an https URL on that audience's own host.
`ironteams-cli audiences` minted all of these live: `api.spaces.skype.com`, `chatsvcagg`, `presence`,
Graph, `substrate.office.com`, `substrate.office.com/sydney`, `outlook.office.com`, `api.yammer.com`,
`service.flow.microsoft.com`, `tasks.office.com`, `onenote.com`, `ic3.teams.office.com`,
`loki.delve.office.com`, `teams.microsoft.com`, and the user's own SharePoint (OneDrive) host.
Access tokens live about 2 hours (`ttl_s` ≈ 7465 in the "tokens refreshed" log).

## Message actions (`teams_proto::actions`) 2026-11-18

All on the chat service unless noted. Verified live in `48:notes` and a private test channel; test
messages were deleted afterwards.

| Action | Request | Live |
|---|---|---|
| Forward | `POST /v1/users/ME/conversations/<src>/messages/forward` `{targetThreadIds, messageIds (≤5), additionalMessage, templateId:"basic_forward_message_template"}` → `[{statusCode, threadId, messageId}]`. The new message carries `<blockquote itemtype="http://schema.skype.com/Forward">` and `properties.forwardTemplateId`/`originalMessageContext` | ✅ 200 |
| Save / unsave | `PUT …/conversations/<conv>/rcmetadata/<rootId>` `{"s":1\|0,"mid":<msgId>}` | ✅ 200 |
| Saved list | `GET …/conversations/48:saved/messages` | ❌ 404 `LocationLookupFailed` for this tenant, and the Teams web app no longer shows a Saved view. IronTeams keeps its own saved list in the store and still sends the server flag |
| Pin for everyone | CSA `POST/DELETE /api/csa/api/v1/chats/<chat>/pinbar` `{pinbarItemType:"pinmsg", pinbarOperation:"add"\|"remove", …}`; list `GET` same URL (`children[]`, folders nest) | ✅ pin, list, unpin |
| Mark as unread | `PUT …/properties?name=consumptionHorizonBookmark` `"<prevId>;<nowMs>;<cmid>"`. `mark_read` now also writes `"0;<nowMs>;0"`, because a read horizon alone does not clear the bookmark | ✅ 200 |
| Mute | `PUT …/properties?name=alerts` `{"alerts":"false"}` (`"true"` unmutes). The property is a plain on/off flag: no end time, duration or "muted until" field is known in it, in the aggregator's `isMuted`, or in the web client's mute menu. "Mute for 1 hour / 8 hours / until tomorrow 08:00 / 1 week" is therefore kept on this computer (`Local.mute_until` in the `lists` KV) and IronTeams sends the unmute itself when the time passes, checked at start and every 15 s | unit-tested |
| Hide | `PUT …/properties?name=unpinnedTime` `{unpinnedTime:<ms>}` then `historyHiddenTime` `"<ms>"`; unhide `{unpinnedTime:null}`. A hidden chat comes back with its next message | unit-tested |
| Share to Outlook | Graph `POST /me/messages` `{subject, body:{contentType:"HTML", content}}` (Mail.ReadWrite, in the Graph `.default` token per plans/README.md) creates an unsent draft; its `webLink` opens in Outlook on the web. `Client::create_mail_draft` | documented Graph call, not tried live |
| Report this message | **UNVERIFIED**: the call Teams makes was never captured. `report_message_unverified` (backend/media/extras.rs) sends nothing and tells the user to report it in Teams on the web | not built |
| Pin a chat | No separate write: new Teams keeps pinned chats in the Favorites folder (`migratedPinnedToFavorites` in `conversationFolders`, see docs/research/open-source.md). The chat menu's Favorite / Unfavorite is that write | ✅ (Favorites) |

## OneDrive (`crates/onedrive`) ✅ 2026-11-18

All calls go through the auth broker. Graph calls use the Graph bearer (`Files.ReadWrite.All`,
`Sites.ReadWrite.All`). The recycle bin uses the bearer for the user's own `<tenant>-my.sharepoint.com`
host. The bearer is only attached to the audience's own host, and pre-authenticated URLs (upload
sessions, copy monitors, download redirects) get no token. `ironteams-cli drive …` runs the read-only
calls, and `ironteams-cli drive selftest` runs every write below inside one throwaway folder and purges
it afterwards.

**O0 finding.** The plan assumed `/me/drive/recent`, `/me/drive/sharedWithMe` and `/me/insights/used`
return nothing. On 2026-11-18 they all answered with data (recent 140 items, shared 500, insights
2 per page), so Home and Shared use them and fall back to Search only when they are empty or fail.
The internal endpoints that the Teams web OneDrive app calls were **not captured** (no browser
capture was run), and none is needed. If Microsoft retires the Graph endpoints, capture the web app
and swap them in `browse::recent` and `browse::shared_with_me`.

Browse (Graph):

| Call | Use | Status |
|---|---|---|
| `GET /me/drive?$select=id,name,webUrl,driveType` | the user's drive | ✅ |
| `GET /me/drive/root?$select=sharepointIds` | site URL and host of the `-my` site (`siteUrl`) | ✅ |
| `GET /me/drive/root/children?$expand=thumbnails&$top=200`, `GET /drives/{d}/items/{i}/children` | My files and folders; `@odata.nextLink` pages | ✅ |
| `GET /me/drive/root:/<path>` and `…:/children` | open a path (for example `Recordings`; 404 when it does not exist) | ✅ |
| `GET /me/drive/following` | Favorites | ✅ (0 items; one appeared after `follow`) |
| `GET /me/followedSites` | Libraries | ❌ 403 `accessDenied` for this token |
| `GET /me/joinedTeams?$select=id`, `GET /groups/{id}/sites/root`, `GET /groups/{id}/drive` | fallback for Libraries and Quick access: the teams' sites and Documents libraries | ✅ |
| `GET /sites/{id}/drives` | libraries of a site | ✅ |
| `GET /me/drive/recent` | Home / Recent | ✅ 140 items (no `$top`: it answers the whole list) |
| `GET /me/drive/sharedWithMe` | Shared with you | ✅ 500 items, no paging link |
| `POST /search/query` `{requests:[{entityTypes:["driveItem","listItem"], query:{queryString}, from, size, sortProperties?}]}` | search; `sortProperties:[{name:"lastModifiedDateTime",isDescending:true}]` is the recent fallback; `* AND NOT path:"<my site url>"` is the shared fallback | ✅ search and sorted search; the exclusion query is accepted (200, 50 hits) but its result was not compared with the real list |
| `GET …/thumbnails` | small, medium and large URLs; `$expand=thumbnails` on listings gives them in one call | ✅ |
| `GET /me/insights/used?$orderby=LastUsed/LastAccessedDateTime desc&$top=n` | "For you": documents the user opened | ✅ 2026-11-19, pages with `@odata.nextLink`. Lists sites too (`resourceReference.type: microsoft.graph.siteItem`, `resourceVisualization.type: spsite`), which IronTeams drops from the cards |
| `GET /me/insights/trending?$top=n` | "For you": trending around the user | ✅ 2026-11-19. Has `weight` and **no** `lastModifiedDateTime` in practice, although the docs show one |
| `GET /me/insights/shared?$top=n` | "For you" shares, "Shared by you", People | ✅ 2026-11-19, 40 sampled: `sharingType` `Attachment` 32, `Link` 5, `Direct` 3; `sharingReference.type` `microsoft.graph.message` with a `webUrl` on `teams.microsoft.com` (34, a Teams chat) or `outlook.office.com` (3); `resourceReference.type` `microsoft.graph.driveItem` (37) or `microsoft.graph.fileAttachment` (3, mail attachments, no drive item). Lists shares **by** the user too: "Shared by you" is this list filtered on `lastShared.sharedBy.id` = the user's id (67 of 200) |
| `GET /me?$select=id,displayName,mail,userPrincipalName` | tells the user's own shares apart | ✅ |
| `POST /search/query` with `(FileType:jpg OR … OR FileType:mkv) AND path:"<my site url>"`, newest first | "Browse files by Media" | ✅ 50 hits per page, all in My files (`parentReference.driveId` = the user's drive); seen: mp4, png, webm |
| `GET /me/joinedTeams?$select=id,displayName`, `GET /teams/{t}/channels?$select=id,displayName`, `GET /teams/{t}/channels/{c}/filesFolder`, `GET /drives/{d}/items/{folder}:/Recordings:/children` | Meetings: the channel libraries' `Recordings` folders (404 when a channel has none) | ✅ 1 channel recording found, plus 3 in My files. Two calls per channel. `/me/joinedTeams` refuses `$top` (400) |
| `GET /me/onenote/notebooks?$select=id,displayName,links` | proves the Graph bearer reaches OneNote (`Notes.ReadWrite.All`) | ✅ 200 (0 notebooks) |

A search hit is a `driveItem` (with `parentReference.driveId` and `id`) or a `listItem` whose file
name is in `fields.fileLeafRef` and whose drive item is in `driveItem.id`. Remote items (shared
entries, shortcuts) carry the target in `remoteItem`; IronTeams points `DriveItem.id` and `drive_id`
at the target and keeps the stub in `DriveItem.stub`. Delete or rename the stub, never the target.

Act (Graph):

| Call | Use | Status |
|---|---|---|
| `POST …/children` `{name, folder:{}, "@microsoft.graph.conflictBehavior":"rename"}` | new folder | ✅ |
| `PUT …/<parent>:/<name>:/content?@microsoft.graph.conflictBehavior=rename` | upload up to 4 MB | ✅ |
| `POST …/<parent>:/<name>:/createUploadSession`, `PUT` chunks to `uploadUrl` | larger files (shared with chat files, `teams_proto::files::upload_chunks`) | ✅ 4.5 MB; two chunks verified earlier |
| `PATCH …/items/{id}` `{name}` | rename | ✅ |
| `PATCH …/items/{id}` `{parentReference:{id}, name?}` | move inside a drive | ✅ |
| `POST …/items/{id}/copy` `{parentReference:{driveId,id}, name?}` | copy: 202 with a `Location` monitor URL. Poll it with no token and no redirects until `status: completed` + `resourceId` (or a 303 whose `Location` ends in the item id) | ✅ |
| `DELETE …/items/{id}` | delete (goes to the recycle bin) | ✅ |
| `GET …/items/{id}/content` | download; answers 302 to a pre-authenticated URL | ✅ byte-identical round trip |
| folder download | the client walks `children` and builds a zip (`zip` crate) | ✅ |
| `POST …/items/{id}/follow`, `…/unfollow` | Favorites | ✅ (the list shows the change after a delay) |
| `POST …/children` `{name, remoteItem:{id, parentReference:{driveId}}}` | add a shortcut | ❌ 400 `invalidRequest` in 5 body variants (id plus drive id, with `folder`, `sharepointIds`, both, the whole remote item) against a folder shared with the user. Not usable until a working body is captured |
| `GET …/items/{id}/versions`, `POST …/versions/{id}/restoreVersion` | version history | ✅ list, 🧪 restore (a new file has one version) |
| `POST …/items/{id}/preview` `{}` | embeddable `getUrl` | ✅ for docx, xlsx, pptx |
| `GET …/content?format=pdf` | server-side PDF, used to prove the blank Word and PowerPoint files are valid | ✅ |
| `GET …/workbook/worksheets` | proves the blank Excel file opens (an empty sheet gives HTTP 406 for `format=pdf`) | ✅ |

Blank documents (`onedrive::package`) are generated in code: Word (3 parts), Excel (5 parts) and
PowerPoint (11 parts: presentation, master, layout, slide, theme and their relationships). Word and PowerPoint convert to
PDF on the server, Excel opens in the workbook API, and all three preview. Markdown and Text are
empty files.

Share (Graph):

| Call | Use | Status |
|---|---|---|
| `POST …/invite` `{recipients:[{email}\|{objectId}], message, requireSignIn, sendInvitation, roles:["read"\|"write"]}` | share with people | 🧪 (a live call would share a file) |
| `POST …/createLink` `{type:"view"\|"edit"\|"blocksDownload", scope:"anonymous"\|"organization"\|"users", expirationDateTime?}` | link; blocking download is the type `blocksDownload`, valid for view links only | 🧪 |
| `GET …/permissions` | who has access | ✅ |
| `PATCH …/permissions/{id}` `{roles:[…]}`, `DELETE …/permissions/{id}` | change or remove access | 🧪 |

Recycle bin (SharePoint REST on `<tenant>-my.sharepoint.com`, `Accept: application/json;odata=nometadata`):

| Call | Use | Status |
|---|---|---|
| `GET <siteUrl>/_api/web/RecycleBin?$top=500` | list: `Id`, `LeafName`, `DirName`, `Size`, `DeletedDate`, `DeletedByName`, `ItemType` (5 = folder) | ✅ |
| `POST <siteUrl>/_api/web/RecycleBin('<Id>')/restore` | restore to the original place | ✅ |
| `POST <siteUrl>/_api/web/RecycleBin('<Id>')/deleteObject` | delete for good | ✅ |

Deleting a folder puts one folder entry (and, depending on timing, its children) in the bin.

Around files: meeting recordings are the children of `Recordings` in My files (✅ 3 items) and in
each channel's files folder (✅ 1, see Meetings above). Sync is
a text (`onedrive::sync_help`) that points at the `onedrive` client and `rclone`.

Which view uses which call (the OneDrive app of the rail, `crates/ironteams/src/backend/drive.rs`):

| View | Call | Notes |
|---|---|---|
| Home (Recent) | `browse::recent` (`/me/drive/recent`, Search when empty) | the type pills and the filter box work on this list on the client |
| Shared, "Shared with you" | `browse::shared_with_me` (`/me/drive/sharedWithMe`) | a shared folder opens as its own root |
| My files, folders | `browse::children` | breadcrumbs are kept on the client, each crumb is a (drive id, item id) |
| Favorites | `browse::following` | the star on a row calls `follow` or `unfollow` |
| Libraries, Quick access | `followed_sites` (joined teams as the fallback) and `site_drives` | the first drive of each site, at most 12 sites |
| Recycle bin | `bin::recycle_bin`, `restore`, `delete_forever` | SharePoint REST on the `-my` host |
| Meetings | `recordings` | the `Recordings` folder of My files |
| Search | `browse::search` | `driveItem` and `listItem` hits |
| Preview | `ops::download` for images, text and markdown; `ops::preview_url` for Office files and PDFs | the preview URL opens in the browser |

"For you", "Shared by you", People, Media and the channel recordings (`crates/onedrive`, not yet
wired in the UI; `ironteams-cli drive for-you|shared-by-me|people|media|meetings`):

| View | Call | Notes |
|---|---|---|
| Home, "For you" cards | `insights::for_you` (used, trending, shared) | shares with the user first, then used and trending, in turn, without duplicates; drive items only. Reason: "You frequently open this", "Trending around you", "<name> shared this in a Teams chat" (sharing reference on a Teams host) or "<name> shared this with you". `previewImageUrl` needs a SharePoint token: take thumbnails from `Insight::item` with `browse::thumbnails` |
| Shared, "Shared by you" | `insights::shared_by_me` | `Insight::to_drive_item` gives the file list row |
| Browse by People | `insights::people` | the shared insights grouped by `sharedBy`, newest first; falls back to `remoteItem.shared.sharedBy` (else `owner`) of `/me/drive/sharedWithMe` when the insights give nobody |
| Browse by Media | `browse::media(from, size)` | Search over the user's own site, filtered to photo and video extensions |
| Meetings | `around::meeting_recordings` | My files plus every channel's `Recordings`, each with its place (`My files` or `<team> › <channel>`) |

The "New" menu's other items (`create::plan` gives the request, `create::create` sends it;
`ironteams-cli drive new <kind> … --dry-run` prints it without a session):

| Item | Request | Status |
|---|---|---|
| Visio drawing | `PUT …/<parent>:/<name>.vsdx:/content` with a blank VSDX built in code (`package`: document, pages, one page) | 🧪 not created live (by instruction); the package is checked by unit tests. `drive selftest` creates one in its throwaway folder |
| Link | `PUT …/<parent>:/<name>.url:/content` with `[InternetShortcut]\r\nURL=<url>\r\n` (what OneDrive's "Link" saves) | 🧪 not created live; in `drive selftest` |
| OneNote notebook | `POST /me/onenote/notebooks` `{displayName}` on the Graph bearer; the answer's `links.oneNoteWebUrl.href` opens it | 🧪 not created live. Graph places the notebook in the user's notebooks, not in the open folder |
| Forms survey | none: Forms has no API for this client, so IronTeams opens `https://forms.office.com/` | n/a |

O0.1 note: these calls were confirmed read-only with `ironteams-cli drive shape <graph path> [n]`, which
prints an answer's structure with every value redacted (URLs reduced to their host, the tenant label
hidden). The internal endpoints of the Teams web OneDrive app itself were still not captured in a
browser; the Graph calls above answer for every view, so none is needed today.

Pitfalls found live:
- A `reqwest` error message contains the request URL, and upload session and monitor URLs carry a
  temporary `tempauth` token. IronTeams strips the URL from those errors.
- Uploads and the recycle bin call sometimes ran past the client's 60 s timeout during the live runs.
  Retrying worked.

## Search, quick views and shared files (`teams_proto::search`, `views`) 2026-11-19

Live checks ran with `ironteams-cli find …` (counts and shapes only). Message content was never printed.

| Call | Notes | Live |
|---|---|---|
| `POST substrate.office.com/searchservice/api/v2/query` `entityType:"Message"`, `contentSources:["Teams"]`, `query.queryString:"NOT (isClientSoftDeleted:TRUE) AND (<text>)"` plus ` AND clientthreadid:<conv>`, ` AND …FromSkypeInternalId_String:<mri>`, ` AND sent>=YYYY-MM-DD`; `from`/`size` page it | headers `x-anchormailbox: Oid:<oid>@<tenant>`, `Origin`/`Referer: https://teams.microsoft.com`, `X-ClientType: MicrosoftTeams`, `X-ClientScenario: TeamsSearch`. Answer `EntitySets[0].ResultSets[0]{Total, MoreResultsAvailable, Results[]}`; each result has `HitHighlightedSummary` (no highlight markup came back for the queries tried, so IronTeams bolds the words itself), `Source{ClientConversationId, ClientThreadId, DateTimeSent, ConversationTopic, ChannelName, TeamName, From.EmailAddress.Name, MentionsPreview.IsMentioned, Extensions{…ParentMessageId, …FromSkypeInternalId, …ThreadType}}` | ✅ |
| Message id of a hit | `ClientConversationId` is `<conv>;messageid=<id>` but that id is **not** the message's. For chat messages and channel posts the message id is `Extensions.…ParentMessageId` (a misleading name; it equals `DateTimeSent` in ms). For a channel reply both name the thread root, and the reply's own id equals `DateTimeSent` in ms (3 fractional digits; some rows have 2, so IronTeams looks for the nearest reply within 1.5 s). `ThreadType`: `chat`, `topic` (channel), `space`, `meeting`, `streamofnotes` (Notes) | ✅ every hit of 10 found in the chat service |
| `mentions:<oid without dashes>` as the query text | the messages that @mention you; `MentionsPreview.IsMentioned` was true for all 50 of the first page | ✅ |
| Substrate `File`, `Chat` and `TeamsChannel` entities | `File` answers `500` for this account, `Chat` and `TeamsChannel` answered 0 results. IronTeams searches files through Graph search (`onedrive::search`) and chats and channels from the local cache | ❌ not used |
| `GET chat/v1/users/ME/conversations/48:threads/messages?view=msnp24Equivalent\|supportsMessageProperties&pageSize=N&startTime=1` | rows with `properties.activity{activityType:"threads", activityReason: author \| channelFollow \| deleted, sourceThreadId, sourceReplyChainId (the root post), sourceThreadTopic, activityContext.followType: ImplicitFollow}`. Replying to a post in the sandbox channel added an `author` row with `ImplicitFollow` within seconds. The newest row per thread decides; `deleted` means not followed | ✅ read |
| Follow: `PUT …/conversations/<channel>/messages/<root>/properties?name=emotions` `{"emotions":{"key":"follow","value":"0"}}` (follow) or `"1"` (unfollow); `DELETE` with `{"emotions":{"key":"follow"}}` clears it | the post then carries a `follow` emotion with the value (read back from the message properties). Because it is stored as an emotion, IronTeams hides the key `follow` from the reaction chips. **Not confirmed**: `48:threads` did not change within 30 s of an explicit follow or unfollow, so whether Teams' Followed threads view reads this flag was not seen (the Teams UI was not used) | 🧪 |
| `GET substrate.office.com/AllFiles/api/users('OID:<oid>@<tenant>')/AllShared?ThreadId=<conv>&ItemTypes=File&ItemTypes=Link&PageSize=50` | header `x-anchormailbox`. `Items[]{ItemType: File \| Link, FileData{FileName, FileUrl, FileExtension, WebUrl}, WeblinkData{Title, WebUrl}, SharedByDisplayName, SharedByMRIId, SharedDateTime, TeamsMessageId, ParentMessageId, ThreadId}`, plus `HasMoreItems` and `Bookmark` (paging not used). `48:notes` returned 5 files and 22 links; the sandbox channel returned none | ✅ |
| `GET graph/v1.0/teams/<group id>/channels/<channel id>/filesFolder` | the channel's SharePoint folder: `id`, `parentReference.driveId`, `folder.childCount`. Its children are listed with `onedrive::children` | ✅ (the sandbox folder is empty) |
| Discover (Teams' feed of popular posts from your teams) | no endpoint is documented here, in docs/research or in the reference clients. IronTeams's Discover quick view is built locally (`find/discover.rs`): the cached channel posts of the last 7 days, ranked by reactions (not counting `follow`) plus replies, and the view says so | — (no network) |

## Real-time (trouter) ✅

1. `POST go.trouter.teams.microsoft.com/v4/a?epid=<epid>` → `socketio`, `surl`, `ccid`, `connectparams`.
2. `GET {socketio}socket.io/1/?v=v4&<connectparams>&tc=…&con_num=…&epid&ccid&auth=true&timeout=40` → `sid:…`.
3. WebSocket `wss://…/socket.io/1/websocket/{sid}?<same query>`.
4. On `1::`, send `user.authenticate` (Bearer spaces token) and `user.activity`, then register 3
   apps at `teams.microsoft.com/registrar/prod/V2/registrations`. `TeamsCDLWebWorker` must be
   registered last.
5. Ping every 30 s with `5:<n>+::{"name":"ping"}`. Ack every `3:::{id,…}` with
   `3:::{"id":<id>,"status":200,"body":""}`.
6. Bodies may be gzip (`X-Microsoft-Skype-Content-Encoding`), or wrapped in `cp` (gzip+base64) or
   `gp` (base64).
7. The registrar throttles at `HTTP 429 {"retryAfter":"00:02:52"}`. IronTeams keeps the socket open
   and retries after that delay. `trouter.message_loss` arrives right after connecting, so
   IronTeams re-registers at most once a minute and resyncs history.
8. After suspend, the server resets the socket (`Connection reset by peer`). IronTeams reconnects and
   resyncs. This was verified by freezing the process for 3 minutes: a message sent during the
   freeze was in the cache after resume, and a message sent after resume arrived live. Token expiry
   uses wall-clock time, because a monotonic clock stops during suspend.

## Built-in apps, read only (`planner`, `workflows`, `onenote`, `lists`) 2026-11-19

`ironteams-cli tasks | flows | onenote | lists` run these. All calls are GETs. The tenant is a work
tenant, so nothing was created, edited or run. Probed live with the auth broker.

| App | Call | Live |
|---|---|---|
| To Do | Graph `GET /me/todo/lists`, `/me/todo/lists/{id}/tasks` (`dueDateTime` is a `dateTimeTimeZone`, read as UTC) | ✅ 200, 1 list, 0 tasks |
| Planner | Graph `GET /me/planner/tasks`, `/me/planner/plans`, `/groups/{id}/planner/plans`, `/planner/plans/{id}/{tasks,buckets}` | ✅ 200, but 0 tasks and 0 plans, so only the shapes from the Graph docs back the parsers |
| Planner, `tasks.office.com` token | `/taskAPI/beta/me/{tasks,lists}`, `/taskAPI/v1.0/me/plans` → 404, `/api/v1/me/tasks` → 503 | ❌ 3 attempts, no usable path. IronTeams uses Graph instead. `/me/planner/favoritePlans` and `rosterPlans` → 400 |
| Workflows | `GET https://api.flow.microsoft.com/providers/Microsoft.ProcessSimple/environments?api-version=2016-11-01`, `…/environments/{env}/flows?…`, `…/flows/{flow}/runs?…&$top=N` | ✅ 200: 1 environment, 4 flows, runs with `status`, `startTime`, `endTime`, `trigger.name` |
| OneNote | Graph `GET /me/onenote/notebooks`, `/groups/{id}/onenote/notebooks`, then the `sectionsUrl`, `sectionGroupsUrl`, `pagesUrl` and `contentUrl` links each object carries; `/pages/{id}/content` answers HTML, not JSON | ✅ notebooks: 0 personal, 2 in the teams' groups. Both notebooks have 0 sections and 0 section groups, so sections, pages and page content were **not** checked live |
| OneNote, `onenote.com` token | `GET https://www.onenote.com/api/v1.0/me/notes/{notebooks,sections,pages}` | ✅ 200, empty. Graph covers the same data, so it is not used |
| Lists | Graph `GET /groups/{id}/sites/root`, `/sites/{id}/lists?$select=…`, `/sites/{id}/lists/{id}/columns`, `/sites/{id}/lists/{id}/items?expand=fields&$top=100` | ✅ 2 team sites; each has only the Documents library (no user lists). Columns and items of a library were read and parsed |

**Audience hosts.** The token audience is not always the API host. `Audience::Flow` mints for
`service.flow.microsoft.com` (no DNS record) and serves from `api.flow.microsoft.com`;
`Audience::OneNote` mints for `onenote.com` and serves from `www.onenote.com`. `request_as` now
accepts those two API hosts as well, and still refuses every other host.

**Team notebooks.** A team's notebook lives under its Microsoft 365 group, so `/me/onenote`
alone finds none. Sections can sit in nested section groups, so IronTeams follows `sectionGroupsUrl`
up to 4 levels.

## Store, app bar and built-in apps: writes, Meeting Recaps 2026-11-20

The screens send these after a confirm dialog (`crates/ironteams/src/backend/appstore/form.rs`).
**None was ever sent against a real account** (build and offline tests only; the demo mode and
the tests log them as `Engine::apps_sent`). "Known" means a Microsoft docs or web client source
names the request; **UNVERIFIED** means the shape is a best reading and a refusal is shown as it
came.

| Action | Request | Source | Status |
|---|---|---|---|
| Add an app (me, team, chat, meeting) | `POST MT/beta/{users\|teams/<id>\|chats/<id>}/apps/entitlements`, body the app definition | web client bundle, plans/store-and-apps.md S2.3 | known, unsent. The team, chat or meeting id comes from IronTeams's own store (the dialog's picker) |
| Remove an app | `DELETE MT/beta/users/apps/entitlements/<appId>` | same | known, unsent. A permanent (built-in) app is refused before sending |
| Update an app | `POST MT/beta/users/apps/Upgrade/<appId>?version=` | same | known, unsent |
| Pin, unpin, reorder | `POST MT/beta/users/apps/pinned`, body `[{appId, appBarOrder}]` | web client bundle, plans S1.1 | known, unsent. The bar changes here first; if Teams refuses, the banner says the change is on this computer only |
| To Do task: new, edit, complete, delete | Graph `POST /me/todo/lists/{id}/tasks`, `PATCH`, `DELETE …/tasks/{id}`; complete is `status: completed`, undo is `notStarted`; the due date is a `dateTimeTimeZone` at 00:00 UTC | Microsoft Graph docs (To Do) | known, unsent |
| Planner task: new, edit, complete, delete | Graph `POST /planner/tasks` (`planId`, `bucketId`, `title`, `dueDateTime`), `PATCH`/`DELETE /planner/tasks/{id}` with `If-Match: <@odata.etag>`; complete is `percentComplete: 100` | Microsoft Graph docs (Planner) | known, unsent. A task without an etag is not edited (the screen asks to refresh). The `tasks.office.com` audience is not used (no usable path, see above) |
| Flow: turn on or off | `POST https://api.flow.microsoft.com/providers/Microsoft.ProcessSimple/environments/{env}/flows/{flow}/start` and `…/stop` (`?api-version=2016-11-01`) | Power Automate management API | **UNVERIFIED**, unsent |
| Flow: run a manual flow | `POST …/flows/{flow}/triggers/manual/run` with `{"inputs": {}}` | the maker portal's Run button; trigger name `manual` is the default | **UNVERIFIED**, unsent. Only a flow with a button trigger that is on offers Run |
| OneNote: new page | Graph `POST <section pagesUrl>` (`/me/onenote/sections/{id}/pages`, or the group's path) with an HTML document, `Content-Type: text/html`; the `<title>` is the page title | Microsoft Graph docs (OneNote) | known, unsent |
| Lists: new item, edit item | Graph `POST /sites/{site}/lists/{list}/items` `{"fields": {…}}`, `PATCH …/items/{id}/fields` | Microsoft Graph docs (lists) | known, unsent. The form covers Title and the first two other editable columns (text, number, choice, yes/no, date); an empty value clears the field on edit |

### Meeting Recaps (`teams_proto::recaps`, plans/meeting-recaps.md R1)

All reads. Rebuilt 2026-11-20 after the live test (MCPS answered 401 and the user's own
OneDrive `Recordings` folder had nothing, because recordings sit in the organizer's OneDrive).
It now follows what the Teams web client shows: the home is the calendar, a recap is the
meeting chat.

| What | Request | Status |
|---|---|---|
| Home list | no request: the online meetings (`isOnlineMeeting` or a join link) of the last 30 days from the calendar IronTeams already syncs (`/me/calendarView/delta`, see Calendar), canceled ones as `Canceled: …`, grouped by week. "All" only: "Followed" has no known source | known (the calendar's own requests) |
| Meeting chat | the chat thread from the event's join link (`/l/meetup-join/19%3ameeting_…%40thread.v2/…`); its messages from the local store, and when none of them is a recording, one page of `GET <chatsvc>/users/ME/conversations/<thread>/messages?pageSize=200` (the existing history call), stored | known |
| Recording parts | chat messages `RichText/Media_CallRecording`: XML `<URIObject type="Video.2/CallRecording.1">` with `<RecordingStatus status="Success">`, `<OriginalName v>`, `<RecordingContent contentTypes duration>` and `<item type="onedriveForBusinessVideo" uri driveId driveItemId>` (or `<a href>`). Only `Success` is a part | shape from the reference clients (rusty-teams `graph_events.rs` and its fixtures); **UNVERIFIED** against a live chat |
| Transcript marker | `RichText/Media_CallTranscript`, content `{\"callId\":…}` (escaped JSON, mautrix `transcripts.go`); or `contentTypes` naming `Transcript` on the recording | reference clients; **UNVERIFIED** |
| Shared files | the `properties.files` of the meeting chat's messages (as the Shared tab reads them) | known |
| Transcript text | SharePoint `GET <tenant>.sharepoint.com/_api/v2.1/drives/<d>/items/<i>/media/transcripts` then `…/transcripts/<id>/streamContent?format=vtt`, for the part's drive item | the reference clients use the same paths; **UNVERIFIED** here. On any failure the tab says so and offers "Open transcript" (the recording's page in the player window) |
| Meeting notes link | Graph `GET /chats/{thread}/tabs?$expand=teamsApp`, the tab named "Notes"; otherwise "No notes" | Graph docs; the tab name is **UNVERIFIED** |
| Watch, Open in browser, Share | Watch (and a recording card's Play) first asks Graph `GET /drives/{driveId}/items/{itemId}?$select=id,size,deleted` (else `/shares/u!<base64url of the link>/driveItem`): 403 says "no access", 404/410 or a `deleted` item says "expired or deleted", 423 or an empty file says "still being processed"; anything else opens the part's SharePoint/Stream link in the web host player window (`webhost`, no tabs or address bar, signed in through the web host profile). Open in browser (More options) uses the system opener. Share copies the link | Graph statuses documented; which one an expired or processing recording gets is **UNVERIFIED** |
| MCPS (optional) | `GET https://teams.microsoft.com/api/mcps/<geo>/user/meetings/contents?…&includeTypes=Recording,TranscriptV2` (host from `mcp_service()`), shape from the mautrix OpenAPI. Sent with only `Authorization: Bearer` for the meeting artifacts resource `6bc3b958-689b-49f5-9006-36d165f30e00` (`Audience::MeetingContent`, minted with IronTeams's client id). The host is `regionGtms.mcpService` from the skypetoken answer; when absent, `<geo>` comes from the regional hosts that name a geography (`chatSvcAggAfd`, `presenceUPS`, `teamsAndChannelsService`, …), then the middle tier (`emea` to `eu`, `amer` to `us`, `apac` to `ap`) | **UNVERIFIED** audience (a live tenant answered 401 on `/mcps/us/` with the old host credentials). Only fills in recordings a chat does not show; any failure is logged at debug level and never shown |
| AI summary, Speakers | not offered: Copilot features | by design |

## Teams and channels (`teams_proto::teams`, `tags`, `engage`) 2026-11-19

Code: `crates/teams-proto/src/{teams,tags,engage}.rs`, CLI `ironteams-cli team …`. Plan:
`plans/teams-and-channels.md` (T0.1 to T3.1).

**How T0.1 was done without a browser.** The web client's JavaScript is public. The shell
`https://teams.microsoft.com/v2/` (fetched with a desktop user agent) lists `runtime-<hash>.js`,
which maps lazy chunk ids to a name and a hash (`a.un`, `a.uh`). The chunk
`services-teams-and-channels-<hash>.js` (22 KB) holds every middle-tier teams and channels call
(url, method, headers); `services-people-targeting` holds the tag calls. Bodies came from the same
chunks and from the open-source clients, then each call was run against a test team. The GraphQL
layer of the web client maps to these calls: for example `showChannel` is two property writes and
`editChannel` is a `PATCH`.

Legend: ✅ run live against the test team (the only team the writes touched), 🧪 body and parser
unit-tested only (not run), ❌ failed live.

`MT` = `regionGtms.middleTier` (`https://teams.microsoft.com/api/mt/<region>`), same auth as the
profile calls. Team and channel ids go in the path percent-encoded. `CSA` = the aggregator.
`chat` = the chat service. `TID` = team thread id (`19:…@thread.tacv2`, also the id of its General
channel), `GID` = the AAD group id (`teamSiteInformation.groupId`).

### T1.1 List
| Call | Notes | Live |
|---|---|---|
| `GET CSA/api/v3/teams/users/me?isPrefetch=false&enableMembershipSummary=true&supportsAdditionalSystemGeneratedFolders=true&supportsSliceItems=true&enableEngageCommunities=false` | `teams[]{id, displayName, description, accessType (3 public), isTenantWide, isFavorite (= shown), isCollapsed, isArchived, isDeleted, memberRoleString (Admin/User/Guest), teamSiteInformation.groupId, membershipSummary{totalMemberCount, adminRoleCount, botCount}, smtpAddress, teamSettings, teamGuestSettings, channels[]}`. `channels[]{id, displayName, channelType (0 standard, 1 private), isShared, isGeneral, isFavorite (= shown), isPinned, isMuted, isArchived, isDeleted, isMember, isModerator, memberRoleString, interest, creationTime, parentTeamId}`. `totalMemberCount` counts the 3 bots every team has, so IronTeams subtracts `botCount`. Channels carry no description and no substrate group id. Pending invites are not in this answer | ✅ |

### T1.2 Discover and join
| Call | Notes | Live |
|---|---|---|
| `GET MT/beta/teams/suggested?accessType=<n>&enableFallback=true` | `accessType` None 0, Private 1, Secret 2, Public 3. `{type, value:[{groupId, displayName, description, accessType:"Public", membersCount, relevantMembers[{smtpAddress}]}]}` | ✅ lists |
| `GET MT/beta/teams/find?keywords=<k>&accessType=<n>&enableKeywordSearch=false&top=100&skipToken=` | same shape. Returned 0 for a one-letter keyword | ✅ lists |
| `POST MT/beta/teams/joinRequest/<GID>` | join a public team, or ask to join a private one | 🧪 (rule: no joins of real teams) |
| `POST MT/beta/teams/cancelJoinRequest/<GID>` | | 🧪 |
| `POST MT/beta/teams/joinByCode?restrictJoinToPublicTeam=<bool>` | body is the code as a JSON string (`"abc1234"`). `{"joinCode":…}` answers `400 Invalid joinCode`. With `restrictJoinToPublicTeam=false` the service answered `400 restrictJoinToPublicTeam query parameter must be set to true` for a private team's code; with `true` it answered `JoinTeamInfo{value{accessType, description, displayName, isTeamOwnerApprovalRequired, …}}`. Tried only with the test team's own code (already a member). IronTeams's join dialog always sends `true`, the only value that worked live | ✅ (own code) |
| `GET/PUT/DELETE MT/beta/teams/<TID>/joinCode` | GET: `204` without a code, else the plain-text code. PUT makes a new code and answers it as plain text. DELETE removes it | ✅ |

### T1.3 Show, hide, pin
| Call | Notes | Live |
|---|---|---|
| Hide a standard channel | `PUT chat/v1/threads/<channel>/members/<me>` `{"expiration":<now s + 14 days>,"interest":"NotInterested"}`, **then** `PUT chat/v1/users/ME/conversations/<channel>/properties?name=favorite` `{"favorite":"false"}`. Sending `favorite` first does nothing | ✅ CSA `isFavorite` flipped |
| Show again | `{"expiration":0,"interest":"Interested"}` then `{"favorite":"true"}` | ✅ |
| Private and shared channels | only the `favorite` property. General: also `isGeneralChannelFavorite` | ✅ private; General 🧪 |
| Hide or show a team | `PUT chat/v1/users/ME/conversations/<TID>/properties?name=favorite` | ✅ |
| `GET/POST/DELETE CSA/api/v1/teams/users/me/pinnedChannels` | GET `{orderVersion, pinChannelOrder[]}`. POST `{"newlyPinnedChannels":[id]}`, DELETE `{"channelIdsToUnpin":[id]}`, with `if-match: <orderVersion>` (retry once on 412) | ✅ pin, list, unpin |
| `PUT/DELETE MT/beta/teams/<channelId>/setDefaultChannel` | "Show for everyone automatically" (the channel id sits where the team id usually goes). Thread property `favDefault` flips `False`/`True` | ✅ |
| Leave a team | `PUT MT/beta/teams/<TID>/members` `{groupId, userMri:<me>, updateType:"Left"}` (same call as removing a member) | 🧪 |

### T2.1 Create a channel
| Call | Notes | Live |
|---|---|---|
| Standard: `POST MT/beta/teams/<TID>/channels` | `{displayName, description, groupId:<GID>, channelType:"Standard"}`; `chatModality:"Conversational"` makes a threaded-chat channel (`PostReply` is posts). Answer `{value:{objectId:<channel id>}}` | ✅ both layouts |
| Private or shared: `POST https://teams.microsoft.com/fabric/<geo>/templates/api/teams/<GID>/channels?forceSync=false` | token audience `https://templates.teams.microsoft.com` (new `Audience::Templates`, only sent to `teams.microsoft.com/fabric/…`), header `X-AnchorMailbox: OID:<my oid>@<tenant id>`. Body `{spaceThreadId:<TID>, displayName, description, channelType:"Private"\|"Shared", sharedWithParentTeam:false}`. The answer has `groupId:"OID:<substrate guid>@<tenant>"`, `hostTeamId`, `hostTenantId`, `chatModalityType`, `createdTime` but **not the thread id**, so IronTeams finds the new channel in the list by name | ✅ private; shared 🧪 |
| `<geo>` | the last segment of the middle-tier URL without its `-NN` (`…/mt/emea` gives `emea`) | ✅ |

### T2.2 Edit, delete, archive, moderation
| Call | Notes | Live |
|---|---|---|
| Edit a standard channel `PATCH MT/beta/teams/<TID>/channels/<c>` | `{displayName?, description?}` | ✅ |
| Edit a private channel `PATCH …/fabric/<geo>/templates/api/teams/<GID>/channels/OID:<substrateGroupId>@<tenant>?forceSync=false` | same body. The substrate id is `properties.substrateGroupId` of `GET chat/v1/threads/<channel>` | ✅ |
| Delete a standard channel `DELETE MT/beta/teams/<TID>/channels/<c>` | body `{id, hostTeamId, isGeneral:false}`. Restorable | ✅ |
| Delete a private channel | `DELETE` the fabric path above | ✅ (it does not appear in the deleted list) |
| Restore `PATCH MT/beta/teams/<TID>/channels/<c>/restore` | no body | ✅ |
| Deleted or archived list `PUT MT/beta/teams/<TID>/allchannels` | `{"Channels":[{"Type":"standard","State":["deleted"]},{"Type":"private",…},{"Type":"shared",…}]}` answers `[{objectId, displayName, description, groupId, channelState, createdDateTime, deleteTime (ms as string)}]` | ✅ |
| Archive `POST MT/beta/teams/<TID>/channels/<c>/archive` | ❌ `404` (also with the group id, an unencoded channel id and a body). The web client's own flag is the thread property: `PUT chat/v1/threads/<channel>/properties?name=archivalLevel` `{"archivalLevel":"Topic"}` archives (CSA `isArchived` turns true), `"None"` unarchives (an empty string is refused). Right after archiving a private channel the unarchive answered one transient `500`, then worked | ✅ property route |
| Moderation (who posts, replies, pins; bots; connectors) | thread property `channelSettings`: `PUT chat/v1/threads/<channel>/properties?name=channelSettings` `{"channelSettings":"{\"channelPostPermissions\":\"Moderators\",\"allowReplies\":\"Everyone\",\"allowPinPosts\":\"Everyone\",\"allowBotsPost\":true,\"allowConnectorsPost\":true}"}` (a JSON **string**). Read it back from `properties.channelSettings` of `GET chat/v1/threads/<channel>`. `PATCH …/channels/<c>` with `channelSettings` answers `500 updateChannelContract cannot be null`. Values: `Everyone`, `Moderators`. The service stores it; enforcement was not tried with a second account | ✅ stored and read back (standard and private) |

### T2.3 Members
| Call | Notes | Live |
|---|---|---|
| Roster `GET chat/v1/threads/<TID or channel>/members?view=msnp24Equivalent&pageSize=100&selectMemberRoles=Admin\|User\|Guest` | `{members:[{id (mri), role:"Admin"\|"User"\|"Guest"}], totalMemberCount}`. Bots have `28:` ids. `GET …/threads/<id>` alone lists only the first member | ✅ |
| Add or change role `PUT MT/beta/teams/<TID>/bulkUpdateRoledMembers?allowAsyncAddition=true` | `{users:[{mri, role: 0 member, 1 guest, 2 owner}], groupId:<GID>}` | ✅ add (member). Role to owner and back answered `200`; the roster did not show the owner within 4 s and the owner count was not re-read before reverting, so 2 = owner comes from the open-source clients |
| Remove or leave `PUT MT/beta/teams/<TID>/members` | `{groupId, userMri, updateType:"Left"}` | 🧪 |
| Private channel members `POST fabric …/channels/OID:<substrate>@<tenant>/users?forceSync=false` | `{users:[{id:"OID:<user oid>@<tenant>", role: 1 member, 2 owner}]}`; `DELETE …/users/OID:<oid>@<tenant>?forceSync=false` | 🧪 (would notify a colleague) |
| Pending requests | `POST MT/beta/teams/findJoinRequests` (owner view of join requests). Nine read-only tries against the test team failed: the body must be a `FindJoinRequestsContext` object (a JSON array answers `400 Cannot deserialize … FindJoinRequestsContext`), and an object with `groupList` as ids, as `{groupId}` objects, with `skipToken`, or wrapped in `findJoinRequestsContext` all answer `500 groupList cannot be null`. The property that holds the groups is not named `groupList` at the top level; not found | ❌ body unknown |
| Guests | `joinRequestAdminAction`, `…/inviteAndAddGuest` exist in the web client; not wired | not done |

### T2.4 Tags (CSA `teams/<TID>/tags`)
| Call | Notes | Live |
|---|---|---|
| List `GET CSA/api/v1/teams/<TID>/tags?tagType=Team&includeTeamTagSettings=true` | array of `{tagId, tagName, tagDescription, memberCount, memberIds[], currentMemberIsPartOfTag, scheduledMembers, tagType:"team", version}` | ✅ |
| All teams `GET CSA/api/v1/teams/users/me/teams/tagCards?tagType=Team&pageSize=500` | `{continuationToken, teamTagCards:[{teamId, teamName, tagCards[]}]}` | ✅ |
| Create `POST CSA/api/v1/teams/<TID>/tags` | `{tagName, tagDescription, memberIds:[mri], showInSection:false}`; the answer is the tag | ✅ |
| Rename `PATCH CSA/api/v2/teams/<TID>/tags/<id>` | **v2**, `if-match: <version>`, `{tagName?, tagDescription?}`. On v1 it answers `400 Unexpected character … '{'` | ✅ |
| Delete `DELETE CSA/api/v1/teams/<TID>/tags/<id>` | | ✅ |
| Members `GET …/tags/<id>/members?pageSize=200` | `{members:[{memberId}], totalMembersCount}` | ✅ |
| Add people `POST …/memberTags?tagType=Team&action=add` | `{memberIds, tagNames}` (by tag **name**) | ✅ (myself) |
| Remove people `POST …/memberTags?tagType=Team&action=delete` | `{memberIds, tagIds}`. Removing the last member deletes the tag | ✅ |
| Mention | mri `tag:<tagId>`, `mentionType:"tag"` (`Tag::mention`, see `mentions.rs`). The composer's @ picker in a channel lists the team's tags (fetched once per team with the List call) and sends them in this shape | 🧪 UNVERIFIED (not sent live) |

### T2.5 Channel email address
`GET MT/beta/channels/<c>/email` answers `404` until an address exists, then
`{allowedSenders:{allowedSenderType:"members"}, emailAddressDetails:{emailId:"<id>.<tenant domain>@amer.teams.ms"}}`.
`POST` creates it (`{"allowedSenderType":"Anyone"}`) and answers that object. `PUT` changes who may
write (`Anyone`, `Members`, `Domains` with `allowedDomains:[…]`; other names answer `500 Error
converting value`) and answers empty. `DELETE` removes it. ✅ all four.

### T2.6 / T2.7 Team
| Call | Notes | Live |
|---|---|---|
| Name check `POST MT/beta/teams/validateNaming` | `{displayName, displayNamePrefix:null, displayNameSuffix:null, mailNicknamePrefix:null, mailNicknameSuffix:null, groupId:null}` gives `{value:{mailNickname}}` | ✅ |
| Create `POST MT/beta/teams/create` | `{displayName, description, accessType:1, isTenantWide:false, validationRequired:false}` gives `{value:{siteInfo:{groupId}, skypeThreadId, …}}`. Public is `accessType:3`. "From a group, team or template" is not wired (`createTeamFromTemplate`, `migrateGroup`, `clone` exist) | ✅ private |
| Edit `PATCH MT/beta/teams/<TID>` | `{displayName?, description?, isDiscoverable?}` gives `true`. Needs the thread id (a group id answers `400 Invalid thread id`). An empty body answers `400 Request should have atleast one of the properties set` | ✅ |
| Settings `PATCH MT/beta/teams/<TID>/teamSettings` | `{"spaceAdminSettings":"<JSON string>"}` where the string is a partial `{"userSettings":{…},"guestSettings":{…}}` merged into the stored settings. Keys are the stored ones: `createTopic`, `updateTopic`, `deleteTopic`, `editEnabled`, `deleteEnabled`, `giphyEnabled`, `giphyRating`, `teamMention`, `channelMention`, `installApp`, `uploadCustomApp`, `createPrivateSpace`, … (the GraphQL names such as `createAndUpdateChannel` answer `AdminSettingsParseFailed`). Read: `GET chat/v1/threads/<TID>` → `properties.spaceAdminSettings` (JSON string) | ✅ set and read back |
| Archive `POST MT/beta/teams/<TID>/archive` (`/unarchive`) | ❌ `500 ArchiveUnarchiveEnqueuePsOperationFailed`, with and without `setSpoSiteReadOnlyForMembers`, on a team minutes old and again later. The thread property `archivalLevel` = `Space` flips CSA `isArchived` for the team (`None` back), but that is only the chat-level flag and does not archive the Microsoft 365 group | ❌ middle tier; property ✅ |
| Delete `DELETE MT/beta/teams/<TID>/delete` | | 🧪 (the test team stays) |
| Discoverable, picture, retention labels, renew | `PUT teams/<t>/discoverable`, `POST teams/<t>/profilepicture?threadId=`, `GET teams/<t>/retentionlabels`, `POST teams/<t>/renew` exist in the web client; not wired (`isDiscoverable` also goes through the team `PATCH`) | not done |

### T2.8 Channel notifications
Write: `PUT chat/v1/users/ME/conversations/<channel>/properties?name=channelNotificationSettings`
`{"channelNotificationSettings":"{\"allNewPosts\":\"On\",\"dskNotif\":\"On\",\"includeReplies\":true}"}`
(JSON string; `On` + `dskNotif` = banner and feed, `On` = feed, `Off` = off). Read it back from
`properties.channelNotificationSettings` of `GET chat/v1/users/ME/conversations/<channel>?view=msnp24Equivalent`
(also `isfollowed`). ✅ all three levels. The batch read on `uis.teams.microsoft.com` needs one more
audience and was not needed. Muting a channel uses the chat `alerts` property of `actions.rs`.

### T2.9 Storyline, T3.1 Communities (Viva Engage, `Audience::Yammer`, `https://api.yammer.com/api/v1`)
| Call | Notes | Live |
|---|---|---|
| `GET groups.json?mine=1` | communities: `{id, name, full_name, description, privacy, company_group, stats{members, last_message_at}, web_url}` | ✅ 1 community (the company-wide one) |
| `GET messages/in_group/<id>.json?limit=<n>&older_than=<id>` | `{messages:[{id, thread_id, sender_id, replied_to_id, created_at ("2026/09/20 08:53:00 +0000"), body{plain, rich}, liked_by{count}, attachments[], web_url}], references:[users and threads], meta}`; sender names come from `references` | ✅ |
| Storyline post `POST messages.json` | ⚠️ body `{"body":"…","is_storyline_post":true}` is a guess the Engage client never uses; with no `group_id` it may land in All Company. Do not send. The real call is GraphQL `createUserWallMessage` on `engage.cloud.microsoft/graphql`, with `isDraft` for an unpublished draft: see `docs/research/storyline.md` | ⛔ |
| `engageCommunities[]` in the CSA v3 list | empty for this account, so the web client's own list is not used | ✅ |
| Storyline tab card `GET users/by_email.json?email=<email>` (`Client::engage_user`) | first user of the array: `{id, full_name, stats{followers, following, updates}}`; the counts of the 1:1 chat's Storyline tab. Read only. From the public Yammer REST API, not captured from the Teams or Engage client | **UNVERIFIED**: not sent to any account |
| Storyline tab posts `GET messages/from_user/<user id>.json?limit=<n>` (`Client::engage_user_posts`) | same shape as `messages/in_group` (`parse_feed`). It lists the person's posts, which may include community posts and not only storyline ones; the tab says "Recent Viva Engage posts". Read only | **UNVERIFIED**: not sent to any account |

Communities are read-only in IronTeams; replying or reacting is not wired.

### Test team
The writes ran against one private team created by `ironteams-cli team sandbox create`; its ids live
outside the repo (`IRONTEAMS_SANDBOX_FILE`). One colleague was added to it, as a member. Everything
else created for the tests (channels, tags, addresses) was removed again except one standard
channel named "tests". The CLI refuses to write anywhere else: every write targets the sandbox
unless `--team <thread id> --group <group id>` is given, and a channel id must belong to that team.

## Chat list, channel posts and cards (plan 8.1 to 8.8) 2026-11-19

Code: `crates/teams-proto/src/{sections,channel_files,posts,chat_card}.rs`, the engine side in
`crates/ironteams/src/backend/{lists,channels}.rs`. Legend as above; the sandbox is the private test
team of "Test team" (channel `tests`), plus the notes chat.

### Chat list sections (8.1)
| Call | Notes | Live |
|---|---|---|
| `GET CSA/api/v1/teams/users/me/conversationFolders?supportsAdditionalSystemGeneratedFolders=true&supportsSliceItems=true` | `{conversationFolders[], conversationFolderOrder[], folderHierarchyVersion, migratedPinnedToFavorites, supportedSlicesMetadata}`. A folder is `{id (guid), name, folderType, sortType, isExpanded, isDeleted, version, createdTime, conversationFolderItems[]}`; an item is `{conversationId, threadType (chat, meeting, space, topic, streamofnotes), createdTime, lastUpdatedTime}`. `folderType` is `Favorites`, `UserCreated`, or a system folder (`RecentChats`, `TeamsAndChannels`, `MeetingChats`, `MutedChats`, `QuickViews`, `EngageCommunities`) which holds no items of its own. `conversationFolderOrder` lists every folder id in display order. `v2` answers the same under `conversationFolderResponse` | ✅ 5 own sections, Favorites with 9 items |
| `POST` / `PUT /<id>` / `DELETE` / `DELETE /<id>` / `PATCH batchUpdateExpandedState` on the same URL | create, update (rename, items), delete items, delete a section, open or close sections. The web client's `services-chat` and its CSA transport define them, but the request bodies are built by its GraphQL layer (`createConversationFolder`, `moveItemToConversationFolder`, `reorderConversationFolders` mutations), which is not in the public chunks | not run: a probe would have changed the real account's sections |

IronTeams reads the sections and keeps its own changes locally (sections it creates, moves, hidden chats,
list options) in the store's key/value table under `lists`. A local move wins over what Teams has.
Writing the sections back needs a capture of the web client's mutation requests (DevTools), see
the plan item.

### Chat item menu and list state (8.2, 8.3)
- Mute: `PUT chat/v1/users/ME/conversations/<id>/properties?name=alerts` (`teams_proto::actions::set_muted`, from 5.x). The state is read from the aggregator: `chats[].members[].isMuted` of our own member entry (`ChatMeta::muted_by`), channels from `isMuted`. The flag has no end time, so a timed mute ("Mute" submenu: 1 hour, 8 hours, until tomorrow 08:00, 1 week) is a normal mute plus a local end time per chat (`Local.mute_until`); `Engine::lists_expire_mutes` runs the same path as Unmute once the end has passed. The menu shows "Unmute (muted until 14:30)". A chat muted and unmuted in another client keeps no end time once unmuted here; one unmuted elsewhere keeps its saved end time until it passes (the unmute then changes nothing).
- Hide: `unpinnedTime` property (`set_hidden`); IronTeams also keeps a local "hidden until a newer message".
- Meeting chats: `chats[].chatType` is `meeting` and the chat service kind is the meeting thread (`ConvKind::Meeting`).
- Chat link: `https://teams.microsoft.com/l/chat/<id>/0`. Channel link: `https://teams.microsoft.com/l/channel/<id>/<name>?groupId=<group>&tenantId=<tenant>`.

### Channel menu (8.5)
Reuses the T1.3 and T2.8 calls (`set_channel_shown`, `set_channel_notifications`,
`channel_notifications`, `remove_private_channel_member`). Hidden channels come from
`teams[].channels[].isFavorite == false`.

### Files in channels and attached cloud files (8.6, 8.7)
| Call | Notes | Live |
|---|---|---|
| `GET graph/v1.0/teams/<groupId>/channels/<channelId>/filesFolder` | drive item of the channel's folder: `id`, `name`, `parentReference.driveId`. `groupId` is `teamSiteInformation.groupId` of the CSA v3 list | ✅ sandbox |
| `PUT graph/v1.0/drives/<driveId>/items/<folderId>:/<name>:/content?@microsoft.graph.conflictBehavior=rename` | up to 4 MB; larger files use `createUploadSession` on the same path. Then `GET drives/<d>/items/<id>?$select=id,name,webUrl,sharepointIds` and a message with `properties.files` as for a chat | ✅ a 25-byte file posted to the sandbox channel, read back as a file card, then the post and the drive item were deleted |
| Attach a cloud file | `GET drives/<d>/items/<id>?$select=…`, then in a chat `POST drives/<d>/items/<id>/invite` for the other members (any drive, not only `me`), then the same message | ✅ an existing OneDrive file posted to the notes chat, then deleted. The invite is unit-tested only (it would share with a colleague) |
| Picker | `GET me/drive/recent`, `GET me/drive/sharedWithMe` (as in `onedrive::browse`), My files and the team libraries | ✅ |

Not verified: that a second account opens a channel file. The folder is the one all channel members
already see.

### Channel posts (8.4)
| Call | Notes | Live |
|---|---|---|
| Announcement | `POST …/conversations/<channel>/messages` through `delivery::Delivery::Announcement` (`send_message_delivery`): `properties.title` = the headline, `properties.cards` = a JSON **string** of one card `{appId:"announcement-card", cardClientId, contentType:"application/vnd.microsoft.teams.messaging-announcementBanner", content:{colorTheme:"<0..11>"}}`, and `properties.subject`. IronTeams offers five colors (theme 0, 1, 2, 3 and 5). The theme palette of Teams was not captured, so the banner is drawn in an approximate color | ✅ stored and read back with title, theme and subject, parsed by `delivery::parse_announcement`. How Teams draws the banner was not seen |
| Follow a thread | `PUT chat/v1/users/ME/conversations/<channel>/rcmetadata/<root id>` `{"f": 1}` (0 unfollows). `{"f": true}` answers `400 Unexpected character`. Read back with `POST …/getrcmetadata` `{"rootMessageIds":[<root>]}` → `rcMetadataList[].rcMetadata.f` | ✅ 1 then 0 |
| Also send to channel | not found in the web client's public JavaScript (no property or call by that name), so IronTeams does not offer it | not built |
| Pin a post | **UNVERIFIED**: the web client reads pinned posts; the write call was not captured. The message menu offers "Pin for everyone" on posts; `pin_channel_post_unverified` (backend/actions.rs) sends nothing and says so | not built |

### Adaptive cards (8.8)
`teams_proto::chat_card` turns the card JSON into a flat list of rows (text with size, weight,
color and subtle; fact rows; column sets with auto/stretch/weighted cells; images; action buttons;
container `style` as a band color on its rows; the announcement banner). `Action.OpenUrl` opens the
browser, every other action is drawn disabled. The raw attachments are stored with the message (key
`c` of the stored extras), so cards are drawn from the structure and the flattened text stays the
preview.

## Top bar gaps: status duration, profile photo, pause durations (feat/gaps-global) 2026-11-20
| Call | Notes | Live |
|---|---|---|
| Status ends after a duration | `PUT <presence>/v1/me/forceavailability/` with `desiredExpirationTime` (UTC `…T…Z`) next to `availability`. "Reset status after" in the profile menu is 30 minutes, 1 hour, 2 hours, today (midnight), this week (midnight before next Monday) or a typed time, counted in the zone chosen in Settings. Documented by the reference clients (`docs/research/open-source.md`), body built by `presence::force_availability_body` | ⚠️ unverified live: IronTeams never sent the expiry field to a real account |
| Status message ends | `PUT <presence>/v1/me/publishnote` `{"message":"<escaped text>[<pinnednote></pinnednote>]","expiry":"<UTC>"}`; the suffix is "Show when people message me", `expiry` is the "Clear after" choice (never = `9999-12-31T23:59:59.999Z`). `presence::status_note_body` | ⚠️ unverified live for a real expiry and for the suffix |
| Pause notifications for a time | the same `toastSettings.temporarilyPauseUntil` write as before (plans/chat.md 9.1); the value is now now+1 h, now+2 h, 08:00 the next day or a typed time instead of always +1 day | ✅ the property; ⚠️ the new durations were not sent live |
| Change profile photo | `PUT https://graph.microsoft.com/v1.0/me/photo/$value`, body = the JPEG or PNG bytes, `Content-Type` image/jpeg or image/png, at most 4 MB (`teams_proto::photo`). Needs the Graph `User.ReadWrite` scope; the `.default` Graph token of the app (plans/README.md) may not carry it, in which case the answer is a 403 that the profile menu shows as it is | **UNVERIFIED**: not sent to any account |
| Remove profile photo | `DELETE https://graph.microsoft.com/beta/me/photo` (Graph documents the removal on beta only) | **UNVERIFIED**: not sent to any account |
| My photo read | the same `profilepicturev2` read as every avatar (`mt::avatar`), for my own MRI, once at start and after a change | ✅ (the read path of all avatars) |

### Files tab actions, channel tabs, meeting pages and call settings (gaps)
Built from the calls and endpoints above. **UNVERIFIED** means no request was sent to a tenant.
| Feature | Call | Status |
|---|---|---|
| Files tab: Download, Rename, Delete, New folder, New document, Share, Copy link | the OneDrive crate's `ops::download`, `rename` (PATCH), `delete`, `create_folder`, `create_office_doc` and `share::create_link` (organization view link) on the drive items of the channel's `filesFolder`. Only rows listed from that folder have a drive item; a file that came from a message offers Open and Copy link only | calls verified in the OneDrive module; **not exercised** from the Files tab |
| Rename a tab | `PATCH graph/v1.0/teams/<g>/channels/<c>/tabs/<tab>` `{"displayName"}` (Graph "Update tab") | **UNVERIFIED** |
| Remove a tab | `DELETE graph/v1.0/teams/<g>/channels/<c>/tabs/<tab>` | **UNVERIFIED** |
| Add an app's tab | `POST …/tabs` `{"displayName", "teamsApp@odata.bind": ".../appCatalogs/teamsApps/<appId>"}` with no `configuration`. Apps come from the user's installed apps (`aggregatedEntitlements`) that work in a team and have tabs. Graph may refuse apps whose tab needs setup | **UNVERIFIED** |
| Add a tab to a chat ("+" in a chat header) | `POST graph/v1.0/chats/<chat>/tabs` with the same body as a channel tab (website or `teamsApp@odata.bind`), `Client::add_chat_website_tab` / `add_chat_app_tab`. IronTeams does not list a chat's tabs, so it says to open the chat in Teams to use the tab | **UNVERIFIED** (never sent) |
| Manage apps (chat item menu) | none: no read of the apps installed in one chat is known (`aggregatedEntitlements` is per user). The item says so (`lists::MANAGE_APPS_NOTE`) | **UNVERIFIED**: not available |
| Out of office (Settings › General) | would be Graph `PATCH /me/mailboxSettings` `automaticRepliesSetting` (needs `MailboxSettings.ReadWrite`, not confirmed for IronTeams's token). Shown greyed out | **UNVERIFIED**: not built |
| Translation preferences (Settings › General) | a Teams account setting; its write is not known. Shown greyed out | **UNVERIFIED**: not built |
| Call forwarding status (Calls page) | the same unknown service as Settings › Calls; the Calls page shows "Forwarding: unknown" and opens Settings › Calls | **UNVERIFIED**: not read |
| Meeting options | no call: the organizer's page `https://teams.microsoft.com/meetingOptions/?organizerId=<Oid>&tenantId=<Tid>&threadId=<thread>&messageId=0&language=en-US`, built from the join URL's thread and `context` (`Tid`, `Oid`), opened in the browser (shape from `docs/research/calendar-meetings.md`) | **UNVERIFIED** (page not opened) |
| Meeting notes and Recap | no call: links found in the event body or preview. Notes are a Loop address or a `.loop` file on SharePoint; the recap is a Teams address that names a recap. Where Teams puts them in an event is not documented, so an event without such a link shows no entry | **UNVERIFIED** |
| Attendance report | not built: Graph `OnlineMeetings` is refused for this client and no web address is documented | skipped |
| Delete a voicemail | `DELETE graph/v1.0/me/messages/<id>` (the voicemail is a mail item); Mark as unread is the existing `PATCH {"isRead": false}` | **UNVERIFIED** (delete); read PATCH verified |
| Call back a voicemail | the caller's mail address is looked up in the directory (`search_people`) and dialled as a 1:1 call | **UNVERIFIED** live |
| Call forwarding, unanswered redirect, ring time, voicemail greeting | the endpoint is not documented in `docs/research/`, `plans/` or the reference clones (the fields would mirror `Get-CsUserCallingSettings`). Settings › Calls shows the controls disabled and says so | not built |
| Dial a phone number | the calling crate cannot place PSTN calls. The Calls search box shows a disabled "Call <number>" row with the reason | not built |
| Ringtone, hide a call, speed dial | local only: `calls.json` (`ringtone`) and `config.json` (`hidden_calls`) | n/a |

## Idle Away (fix/idle-away-live) 2026-11-25
| Call | Notes | Live |
|---|---|---|
| Report idle / active | The device is registered with `PUT <presence>/v1/me/endpoints/` `activityReporting: "Transport"`, which makes the service take the activity from the trouter: socket.io event `user.activity` `[{"state":"active"\|"inactive","cv":…}]` on the connection whose `epid` is the endpoint's (rusty-teams `realtime/native.rs`, Teams web). IronTeams sent `active` once per connect and never `inactive`, so the user stayed Available while idle. Now `trouter::set_user_active` sends every change (both trouter connections), and `POST me/reportmyactivity/` (the `"Direct"` reporting path) is still sent for its 404 | ⚠️ unverified live: the user's next idle test (log lines `trouter: user activity sent active=false`, then `presence: mine as the service shows it availability="Away"`) |

Docs synced to commit 09fe5c2 ("feat(IronTeams): chat list sections and menus, channel post cards, cloud attach and adaptive cards (chat 8.1 to 8.8)", 2026-11-19)
