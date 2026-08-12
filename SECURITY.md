# Security Policy

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security vulnerabilities.

Report privately so we can assess and fix before disclosure:

1. Email the maintainers via the contact listed on the
   [GitHub organization / repository](https://github.com/blocksdevpro/boris-assistant)
   (owner profile or security contact), **or**
2. Use [GitHub private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
   on this repository if it is enabled.

Include:

- A clear description of the issue and impact
- Steps to reproduce (PoC if possible)
- Affected version / commit if known

We will acknowledge reports as soon as practical and coordinate a fix and
disclosure timeline. Early project — response times may vary.

## Secrets and local data

| Location | Contents | Notes |
|----------|----------|--------|
| `.env` (repo root, gitignored) | Dev API keys, model ids | Do not commit |
| `~/.boris/auth.json` | API keys and other secrets | **Plaintext** JSON on disk |
| `~/.boris/config.toml` | Non-secret prefs | Safe to inspect |
| `~/.boris/logs/` | Desktop diagnostics | Avoid logging full keys |

**`auth.json` is plaintext secret storage.** Protect the user home directory with
OS permissions. Boris does not currently encrypt secrets at rest. Treat
`~/.boris` as sensitive.

Never paste live API keys into issues, PRs, or logs. Prefer redaction in any
new tracing around settings load.

## Agent tools and shell (honest threat model)

Boris is a **desktop assistant with tool use**, not a hardened multi-tenant
sandbox.

- **Capability presets** (`voice_safe` / `local_power` / `full`) and path
  sandboxes constrain workspace roots; they are policy layers, not a VM or
  container isolation boundary.
- **Shell (bash tool):** human-in-the-loop (**HITL**) confirmation and host
  `ShellPolicy` are the **authoritative** controls. A small **command deny-list**
  blocks only obviously catastrophic substrings and is **best-effort** —
  easy to bypass with encoding, alternate spellings, or indirection.
- On Windows, when Git Bash is missing, the tool may fall back to PowerShell
  with execution-policy bypass for usability. That is **not** a sandbox.
- **Web fetch** blocks common private/link-local/metadata hosts; residual
  DNS-rebinding and similar risks may remain.

**Do not claim or assume a fully sandboxed shell.** Prefer HITL for high-risk
actions and keep capability presets appropriate to the user’s trust level.

## Scope notes

- The product host is **Windows-first** (`boris-desktop`). Security assumptions
  about packaging (ORT/DirectML DLLs, wake ONNX at build time) are documented
  in the root [README](README.md) and [desktop/README.md](desktop/README.md).
- Third-party models and native runtimes carry their own licenses and trust
  boundaries; see [THIRD-PARTY-NOTICES](THIRD-PARTY-NOTICES) for pointers.

## Supported versions

| Version | Supported |
|---------|-----------|
| 1.0.x   | Yes |

Security fixes target the currently supported 1.x release on `main`. There is
no long-term-support branch at this time.
