---
name: spec
description: Interview user in depth and write a detailed spec to a file
---

# Spec Interview & Writing

Read the spec file the user named, if it exists, (it may be empty or contain a draft), then conduct an in-depth interview with the user to flesh out a complete specification.

## Instructions

### Core Principle: Explore Before Asking

**NEVER ask the user a question that could be answered by exploring the codebase.** Before formulating ANY question, first search the code to see if the answer already exists in the code. Only ask when:
- The information genuinely doesn't exist in the codebase
- It's a product/business decision that code can't answer
- You need to confirm a deviation from existing patterns

### Steps

1. **Read the spec file** to understand any existing context or draft

2. **Explore the codebase first** (a read-only sub-agent is a good fit). Look at:
   - Existing architecture and module structure relevant to the feature
   - Similar features or patterns already implemented
   - Data models, types, and state management patterns in use
   - Error handling conventions and patterns
   - Testing patterns and infrastructure
   - Integration points and boundaries between modules
   - Any existing code that would be modified or extended

3. **For each potential question, apply this filter:**

   ```
   Before asking: "How should we handle X?"

   1. Explore: "How does the codebase currently handle X or similar cases?"
   2. If pattern exists → Don't ask, just note "Will follow existing X pattern from module Y"
   3. If no pattern exists → Ask, but frame it: "The codebase doesn't have a pattern for X yet. How should we approach this?"
   ```

4. **Only ask questions the codebase cannot answer:**
   - Product scope decisions (what's in/out of scope)
   - Business logic that isn't yet implemented
   - Tradeoffs where multiple valid approaches exist AND no precedent is set
   - Explicit deviations from existing patterns (with justification)
   - Performance/resource constraints not documented in code
   - External system behaviors not visible in the codebase

5. **When you do ask, show your exploration work:**
   - "I explored X and found [pattern/nothing]. Given that, [question]?"
   - Never ask generic questions - always ground them in what you discovered

6. **Iterate: each user answer triggers more exploration**
   - User mentions a module → Read that module before follow-up questions
   - User describes desired behavior → Look for existing similar behavior
   - User names a constraint → Look for how other code handles that constraint

7. **Write the final spec** back to that file with:
   - Overview and goals
   - Non-goals (what this explicitly does NOT do)
   - Technical design (referencing existing patterns you'll follow)
   - Decisions made (with codebase evidence that informed them)
   - Open questions (only those that genuinely couldn't be answered)
   - Links to relevant existing code (file paths)

The spec should read like you already know the codebase intimately. If a reader thinks "why didn't they just look at how X does it?" - you failed to explore enough.
