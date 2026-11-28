# Using IronTeams from Claude (MCP)

`ironteams --mcp-serve` is a [Model Context Protocol](https://modelcontextprotocol.io) server. An
MCP client such as Claude Code, Claude Desktop or OpenCode starts it and can then use one signed-in
IronTeams account:

- read the chats you chose to share with AI;
- read your calendar and people, if you allowed that;
- draft messages and reactions in chats you allowed it to draft in. Nothing happens until you
  approve the draft.

It runs on stdio, uses the session the desktop app already saved in the keyring, and needs no API
key. It is a mode of the `ironteams` binary, like `--report-problem`: no window opens.

**Status: not in release builds yet.** The server is behind the `mcp` cargo feature of the
`ironteams` crate, which is off by default. Build it yourself to use it:

```sh
cargo build --release -p ironteams --features mcp
```

Without the feature, `ironteams --mcp-serve` only says that the build has no MCP server.

**The controls are not in the desktop app yet.** The per-conversation settings below are read from
a file that the desktop app will write in a later release. Until then nothing can switch a
conversation on, so the server starts and refuses every conversation. Nothing is enabled for you.

## Before you start: where your data goes

Whatever the AI app reads goes to its AI provider: messages, names, meeting text. For Claude, that
provider is Anthropic. For a work account, check that your organization allows this first. Because
of this, **every conversation starts Blocked**, including all that exist today, and so does every
new one. Nothing is copied from the old account allowlist (see Upgrading).

## What you control

Each chat has its own setting, shown in the chat itself (not in Settings):

| Setting | What the AI may do in that chat |
|---|---|
| Blocked | Nothing. It cannot tell the chat's name, text, people or time. |
| Can read | Read its messages, find them in a search. |
| Can read and draft | The same, and draft messages and reactions there. |

Two more switches sit above them:

- **AI access: Active / Paused** stops every tool at once (chats, calendar, people, drafts and
  `send_draft`) and keeps the saved settings, so setting it back to Active brings the same chats
  back. Each account also has its own pause switch.
- **Calendar and people** (on or off, per account) covers the agenda, free-time search, people and
  presence, and is the first of two keys for a meeting's details (below). It is off until you turn
  it on.

A change takes effect on the next tool call, and in the middle of a call that reads several
conversations. Channels are not covered yet and stay Blocked. The AI may not change your own status
or status message in this version.

### Meetings

The setting of a meeting is the setting of its **meeting chat**, and you set it in the meeting's
full event view in the calendar (the same chip as in a chat). A meeting's details and answering an
invitation need both keys:

| Calendar and people | The meeting chat | `meeting_detail` | `draft_meeting_response` |
|---|---|---|---|
| Off | any | Refused (calendar) | Refused (calendar) |
| On | Blocked | Refused (meeting) | Refused (meeting) |
| On | Can read | Allowed | Refused (meeting) |
| On | Can read and draft | Allowed | Allowed |

- The recording and transcript notices inside a meeting chat are messages of that chat, so they
  follow the chat's own setting through `read_chat`; the Calendar and people switch is not involved.
- A meeting with **no Teams chat**, such as one joined by a `/meet/<code>` link, or an event that is
  not a Teams meeting, stays Blocked: its details are refused and the full event view says "AI
  access: not available for this meeting (no Teams chat)". With Calendar and people on, it is still
  visible in `calendar_agenda` (title, time, organizer and place), never its attendees or invitation text.
- A blocked meeting and a meeting with no chat get the same refusal, so the AI cannot tell them apart.
- To find an event's chat, IronTeams has to ask Teams for the event, so the invitation text of a
  blocked meeting is fetched inside the server and thrown away. It is never returned to the AI.

### What the AI sees about conversations it may not use

One sentence at the end of every list: *"N other conversations are not shared with AI; the user can
allow them in IronTeams."* `N` counts the chats of the account that are not shared. It is the same
number for every tool and every query, so it says that blocked chats exist and roughly how many,
and nothing about what any of them holds: no id, name, preview, sender, time or search hit count.
A blocked chat, an unknown id and a channel id all get the same refusal text.

## How the settings are stored

`mcp-access.json` in the config directory (next to `config.json`), written only by the desktop app.
A missing file means the default: Active, nothing enabled. The server reads the file again on every
tool call. If the file is corrupt, unreadable or from a newer IronTeams, **everything is refused**
with a message that says the settings could not be read; the server does not crash.

## Setup

```sh
ironteams --mcp-setup --account <label>
```

This prints what follows and edits nothing. (`ironteams-cli accounts` lists the labels.)

- The Claude Code command:

  ```sh
  claude mcp add --scope user ironteams -- /path/to/ironteams --mcp-serve --account <label>
  ```

- A permission rule that makes Claude Code ask you before every send. Add it to
  `~/.claude/settings.json` and don't answer "always allow" for `send_draft`:

  ```json
  { "permissions": { "ask": ["mcp__ironteams__send_draft"] } }
  ```

- The `mcpServers` entry for Claude Desktop's `claude_desktop_config.json`.
- Deny rules for an AI app that can also run commands (see the next section).

## What this protects, and what it does not

The settings limit what an AI can reach **through the MCP tools**. They protect against a model that
reaches further than you intended, and against a message in an allowed chat that tries to make the
AI read other chats. No MCP tool can change a setting or widen access.

They do **not** stop an AI app that can run commands or read files as your user. That user can read
the local message cache, run `ironteams-cli`, read the sign-in token from the system keyring, or
attach to the app. No storage location fixes this without a separate OS user or a sandbox. If your
AI app can run commands, run it in a sandbox, and deny at least these in Claude Code (a deny rule on
`Bash` alone can be bypassed with another interpreter):

```json
{
  "permissions": {
    "deny": [
      "Bash(ironteams-cli:*)",
      "Bash(sqlite3:*)",
      "Bash(secret-tool:*)",
      "Read(~/.local/share/ironteams/**)"
    ]
  }
}
```

Turning access off stops new reads. It cannot take back what the AI app has already read.

## Tools

Times are UTC. A tool that needs something you have not allowed returns a short refusal.

| Tool | Needs | What it does |
|---|---|---|
| `list_chats` | Chats you allowed | The allowed chats with id, kind, title, read state and last message, optionally unread only |
| `read_chat` | Can read | The messages of one chat, with message and sender ids; pages back with `before_ms` |
| `catch_up` | Chats you allowed | Unread allowed chats with their unread messages, plus the messages in them that mention you |
| `search_messages` | Chats you allowed | Message search over the allowed chats, optionally in one chat or from one person. No totals. |
| `find_people` | Calendar and people | People by name or email, with presence and status message |
| `get_presence` | Calendar and people | Presence of people by id |
| `calendar_agenda` | Calendar and people | Your events for a range of days |
| `meeting_detail` | Calendar and people, and Can read on the meeting's chat | One Teams meeting: attendees, responses, invitation text |
| `find_free_time` | Calendar and people | Free slots for you and the given attendees |
| `draft_message` | Can read and draft | Drafts a message or a quoted reply |
| `draft_reaction` | Can read and draft | Drafts a reaction (like, heart, laugh, surprised, sad, angry) |
| `draft_mark_read` | Can read and draft | Drafts marking a chat read |
| `draft_presence` | not allowed yet | Drafts your status. Refused in this version. |
| `draft_status_note` | not allowed yet | Drafts your status message. Refused in this version. |
| `draft_meeting_response` | Calendar and people, and Can read and draft on the meeting's chat | Drafts accept, tentative or decline; the organizer gets it |
| `send_draft` | The draft's own setting | Does one approved draft, after checking the setting again |
| `discard_draft` | Not paused | Throws a draft away |

## How a write happens

1. A `draft_*` tool freezes exactly what would happen and returns a draft id and a preview: the
   chat (kind, title, member count) and the exact text or change. Nothing is sent.
2. The AI shows you the preview. If you approve, it calls `send_draft` with the id, and Claude Code
   asks you once more because of the `ask` rule.
3. `send_draft` checks the setting **again** and refuses if the chat no longer allows drafting (for
   example you lowered it to Can read after the draft was made). Then it does the frozen action. It
   never uses new parameters from the call. A draft can be sent once, within 10 minutes. Drafts
   live only in the running server. A refused draft is used up.

Messages, names and meeting text come from other people. The server tells the client to treat them
as data, not instructions, and nothing is written without your approval.

## The access log

`mcp-audit.jsonl` in the data directory (`~/.local/share/ironteams/`) gets one line for every tool
call, allowed or refused: the time, account, tool, the decision (`allowed`, `blocked`,
`master_paused`, `account_paused`, `settings_unreadable`), the version of the settings it saw, the
server version, and a count with short hashes of the conversation ids involved. It never holds
message text, titles or previews. If a line cannot be written the call is refused. Every send attempt
also adds a line with the kind of write, the target, the draft id and the outcome (`sent`, `failed`,
`expired`, `unknown_draft`, `refused`).

The desktop app reads this file to show when the AI last read a conversation (only calls that
returned its content count; refused calls do not). The diagnostic bundle has an `mcp.txt` entry:
the state of the switches, the generation of the settings, how many conversations are enabled at
each level, and the last lines of this log. It holds no conversation id, title or user id, and the
log lines in it hold only the short hashes.

## The sign-in

The server never writes the sign-in token back to the keyring. It reads it at start and keeps any
refreshed token in memory only. So a sign-out in the app stays a sign-out while the server runs. The
tools also refuse when the account's settings no longer belong to the signed-in user.

## Upgrading from `ironteams-cli mcp`

`ironteams-cli mcp` (with `allow`, `deny`, `status` and `setup`) no longer exists; it fails like any
unknown command. Update your AI app's setup with `ironteams --mcp-setup --account <label>`. An
account you allowed before is not carried over: every conversation is Blocked until you choose.

What you see if you had allowed an account in the old `mcp.json`:

- On first start the app moves the account names from `allowed_accounts` to
  `legacy_allowed_accounts` in the same file and empties `allowed_accounts`. This happens once and
  enables nothing. An old `ironteams-cli` that still reads the file finds no allowed account and
  refuses to start, so going back to it fails closed. Going back to an older app also loses the
  new settings.
- The server's start-up message on stderr (in your AI app's server log) says how many
  conversations are enabled, which is 0 at first.
- Until you enable a conversation for that account, `list_chats`, `catch_up` and `search_messages`
  say "AI access is active, but no conversation is enabled." above their closing line, so neither
  you nor the AI mistakes an empty list for a fault. The text goes away when one conversation is
  enabled.

## Not in this version

- Channels as their own setting.
- Files and images.
- Editing or deleting messages.
- Scheduled messages.
- Calls.
- Controlling the running desktop app.
