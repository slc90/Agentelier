# Bundled source

Source: https://github.com/openai/codex/tree/7df454ceb5a6f0ef51555ba59f920e9dd2e70dee/codex-rs/skills/src/assets/samples/skill-creator

Upstream revision: `7df454ceb5a6f0ef51555ba59f920e9dd2e70dee`.

The complete sample resource directory is included with its Apache-2.0 license in license.txt. Agentelier adapts SKILL.md for Windows x64, PowerShell 7, standard writable .agents/skills roots, optional Python/PyYAML, and task-local discovery. Python helpers explicitly use UTF-8 for file I/O; the validator accepts standard compatibility metadata and rejects empty required fields and mismatched directory names. Nonessential source comments and docstrings are removed, and description limits are named, to follow repository conventions. Resource paths and helper workflows remain part of the same ordinary Skill; no installer, permissions, or execution engine is imported from Codex.
