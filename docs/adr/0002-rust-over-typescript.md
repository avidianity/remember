# Rust instead of TypeScript, despite the reference TOON library being TypeScript

Remember is written in Rust with the official `rmcp` SDK, even though the reference TOON encoder and the most mature MCP SDK are TypeScript.
Every Agent spawns its own server process, so several run at once for the whole working day: a Rust process costs roughly 3-5MB and starts in about a millisecond, where a Bun process costs roughly 30-50MB.
Rust also ships as one static binary with no runtime to install on each Agent's machine.

## Consequences

- TOON output must match the spec without the reference encoder, so encoder output is tested against the official spec fixtures.
