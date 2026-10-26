---
name: open-in-editor
description: Find the code a plain-language query refers to and open it in the editor at the right line.
---

# Open in editor

1. Search for the query: symbol names, comments, file paths.
2. Pick the single best location. Prefer the definition over a use and the
   implementation over a test.
3. Open it: `cursor --goto <file>:<line>:<column>` (or `zed <file>:<line>`).
4. Say what you opened and why it matches.
