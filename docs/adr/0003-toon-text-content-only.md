# Tool results are TOON text content only, never structuredContent

Every tool returns its result as a single TOON text content block and never sets `structuredContent` or an `outputSchema`.
Returning both would double the tokens, and the clients disagree on which one reaches the model: Claude Code (2.1.285) and Codex (0.150) show only `structuredContent` when it is present and drop the text, while opencode (1.18) shows only the text.
Adding `structuredContent` "for correctness" would silently replace TOON with JSON for two of the three target clients.
