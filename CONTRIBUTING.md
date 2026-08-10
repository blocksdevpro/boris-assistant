# Contributing

Thanks for interest in **Boris Assistant**. This is a Windows-first desktop
voice assistant (Tauri host + Rust workspace). Contributions that match the
project’s architecture and keep PRs reviewable are welcome.

## Project constraints

- **Primary OS:** Windows (mic/speaker, ORT/DirectML packaging, product host).
  Pure library work may build on other OSes; full desktop is not guaranteed.
- **Product entrypoint:** `desktop/` → `boris-desktop`. The root `boris-assistant`
  package is a retired CLI stub — do not extend it as the voice host.
- **License:** Apache-2.0. By contributing you agree your changes are licensed
  under the same terms (see [LICENSE](LICENSE)).
- **Architecture notes:** [docs/design/oss-collaboration-refactor.md](docs/design/oss-collaboration-refactor.md)
  and per-crate `README.md` files under `crates/`.

## Prerequisites

- Rust (stable, edition 2021)
- For desktop only: [Bun](https://bun.sh), [Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/),
  and a wake ONNX at `assets/models/livekit/boris-large.onnx` (see root README)

Dev secrets: copy [`.env.example`](.env.example) to `.env` (gitignored).

## Build and test (Rust workspace)

Library crates (no Tauri UI, no wake ONNX required for the core/agent plane):

```bash
cargo test -p boris-core -p boris-ai -p boris-agent --lib
cargo test -p boris-audio -p boris-sense -p boris-inference --lib
cargo test -p boris-pipeline --lib
cargo check -p boris-pipeline --features stt-parakeet,tts-supertone
```

Full product (needs wake ONNX + frontend toolchain):

```bash
cargo check -p boris-desktop
cd desktop && bun install && bun run tauri dev
```

CI runs the lightweight library suite above (see [`.github/workflows/ci.yml`](.github/workflows/ci.yml)).
Prefer that path for day-to-day PR validation unless you are changing desktop packaging.

## Pull request guidance

- **Keep PRs focused.** Prefer one concern per PR (docs, one crate, or a
  single feature slice). Large multi-crate rewrites are hard to review and
  often delayed.
- **Size:** small/medium is ideal. If a change must be large, split into a
  short series with a clear order in the PR description.
- **Do not** commit secrets, `~/.boris` data, large model weights under
  `assets/`, or unrelated lockfile churn.
- **Match existing style:** clear technical prose, no emoji spam in docs;
  follow neighboring Rust/TS conventions.
- Describe **what** and **why**, link issues when relevant, and note test
  commands you ran.

## Security

Report vulnerabilities privately — see [SECURITY.md](SECURITY.md).

**Shell / tools honesty:** the bash deny-list is best-effort only; HITL and
host shell policy are the real controls. Do not document or market the agent
as fully sandboxed. Details in SECURITY.md.

## Code of conduct

Participation is governed by [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Questions

Open a GitHub Discussion or issue for design questions when possible. For
structural changes (new crates, agent split, license changes), prefer a short
design note or ADR-style write-up before a large implementation PR.
