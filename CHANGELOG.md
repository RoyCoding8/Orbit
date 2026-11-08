# Changelog

## 0.4.1

### Changed

- Kiro V3 refresh: new hook triggers SessionEnd, PreCompact, PostCompact, and Manual, with V3 trigger spelling compatibility. `chat.agentEngine` now defaults to V3 semantics with legacy V2 profile detection. Documentation refreshed for local Kiro Memory and V3 session boundaries.

## 0.4.0

### Added

- Antigravity App and CLI support: skills, instructions, modular rules, hooks, MCP servers, and CLI settings.
- Antigravity chats grouped by project, with on-demand conversation loading and optional indexing for message search.
- Antigravity native chat export, import, restoration, and deletion. Archives include the session database, annotations, and chat-owned artifacts.
- Chat conversion to and from Antigravity, with an optional destination working directory. Incoming conversions create CLI chats and transfer conversation text and supported inline images.
- Selective Antigravity setup export and import, with saved versions in History for replaced resources.

### Compatibility

Antigravity chat import, restoration, deletion, and incoming conversion require `agy` 1.5.0 with the Antigravity App and CLI sessions closed. Restoration preserves the original App or CLI store and chat ID; an existing ID cannot be overwritten. Conversion creates a new chat and leaves the source unchanged.

See [Antigravity support](docs/reference.md#antigravity) for details.
